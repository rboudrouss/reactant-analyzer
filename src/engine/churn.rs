//! The program churn graph (ADR-018, ADR-042 §4): a fold over two relations.
//!
//! ```text
//! edge x → y  ≡  "a change of x re-runs an effect whose write may change y"
//!                                            a cycle = self-sustaining loop
//! ```
//!
//! A write may change its slot unless that is proven impossible
//! (`written::Change`), and an edge exists unless its convergence is proven
//! — the graph no longer looks for growth: a toggle `!on` closes an edge as
//! surely as a counter. What settles a write: it stores the slot's own
//! content; it stores one value known in advance and no other write moves
//! the slot away; it reads neither the trigger nor its own slot
//! (`Invariance::reads`) and crosses no call that may allocate (#218), so
//! the re-run stores what the last run did — unless a read moved in
//! between, which is why every slot such a write reads is an edge into its
//! slot, trigger or not (#204); every
//! write of the slot moves it one way by a constant into a finite interval
//! (`moves_one_way`); or the guard proofs below.
//!
//! Nothing is walked here. The writes are the effect-region rows of
//! `slot_writers`, the re-run triggers the rows of `effect_triggers`, and the
//! graph is the product of the two, per effect, program-wide. An effect whose
//! re-run class (`triggers::Rerun`) is `Always` is read as one with no
//! dependency array: an edge from every slot its write reads. Edge strength:
//! - `Must`: the dep on x is the exact slot (must-rerun), or the effect
//!   surely re-runs after every render (no deps array, a fresh dep) ∧ the fresh write to
//!   y sits in a block on all paths of the body (must-reach) ∧ the written
//!   value is a new reference every call (`Change::Must`) ∧ so is every
//!   other write of y, program-wide: React re-renders on the batch's last
//!   value, and a later `setY(y)` in the same batch undoes the fresh one
//!   (#188) ∧ the component stays mounted: every element that mounts it
//!   keeps it, and its parent stays in turn (#213). An all-must cycle ⇒
//!   Error.
//! - `May`: the dep is merely versioned by x, or the write is conditional,
//!   nested, deferred or imprecise.
//!
//! **The phase column decides what an effect-body write can sustain.** A
//! write the effect runs synchronously carries its block and may be Must. A
//! `Deferred` or `Cleanup` write runs on a later turn: a May edge, and in an
//! effect with no dependency array a May self-edge — the auto-run continuation
//! `.then(() => set(fresh))` re-runs after every render like the body does
//! (#26). A `Handler` write needs a user event per iteration and is not
//! self-sustaining: no edge (ADR-034's registrar proof). ⊤ is May, the
//! fire-more direction.
//!
//! **Convergence kill**: an edge is dropped when the write provably fires at
//! most once in the automatic loop — its dominating guards die once the
//! written value sits in the slot, under its own write and under every
//! other write of the slot program-wide, each taken with the invariant
//! facts that site ran under ([`converges_under_all_writes`], #154). A site
//! in another component has guards this component's env cannot read, so
//! such a slot is never killed; a handler row needs a user event and is not
//! a site. One kill, one proof, for the dep-driven same-slot edge too. The
//! proof reads facts over props only in a component whose props hold: no
//! write the loop can run moves a slot of an ancestor on the component tree,
//! and no effect of it reacts to another component's slot. Otherwise the
//! props may move on any cycle through it, whichever edge the cycle takes —
//! an effect with no deps array re-runs on them with no trigger row (#206).
//!
//! **The props node** `P_c` (#207): its in-edges are the ancestors' slots a
//! write the loop runs moves, so the props hold exactly when none enters
//! it. Out of it go an edge into a write that reads a prop of an effect a
//! prop re-runs, and a mount edge into every effect write of a component
//! that does not stay mounted, which fires again at each mount whatever its
//! certificates say. The node is eliminated: its out-edges are pushed from
//! each in-edge's slot, so every edge keeps its carrying effect.
//!
//! **What is a site** (#162): every non-handler row of an effect, render
//! or memo body — a `setter-in-render` write revives what it revives — and,
//! from a mount-only effect, only the rows that write another component's
//! slot: the component itself stays mounted and fires it once, but a child
//! the loop mounts and unmounts fires its `[]` effect on every round.
//!
//! **Sites converge by least fixpoint** (#160): a site proven to fire at
//! most once revives nothing indefinitely, so it is excluded from the
//! revivers of every other site, and the set grows until nothing more can
//! be proven. The graph then kills an edge when its write's site is in the
//! set, or when the site converges once its write is read as the part of its
//! value that churns (`churning_part`) — a `null` run stores no change.
//!
//! **Two arms, one relation** (ADR-020 item 2). A dep-driven edge whose write
//! lands in the very slot its deps read is `self_slot`: the self-churn arm's
//! partition. The cycle search skips it; the self-churn arm reads only it.
//! Every other edge is the graph arm's.

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{AbstractEnv, Stability, StateValue},
    engine::{
        AnalysisResult, Change, ConvergedEval, EffectTrigger, ProgramAnalysisResult, SlotWriter,
        WriterPhase, WriterRegion,
        dominance::on_all_paths,
        guards::{
            Invariance, Reads, WriteSite, converges_under_all_writes, let_bindings, mutated_roots,
            navigates, site_guards,
        },
        render_deps::written_names,
        setters::{
            memo_val_labels, must_alias_scopes, must_state_vals, resolve_setter_aliases,
            state_val_labels,
        },
        triggers::{Always, Rerun},
        triggers_of,
        written::churning_part,
    },
    ir::{
        ComponentId, QualifiedSlot, SourceRange,
        bindings::local_bindings,
        cfg::{CFG, Terminator},
        expr::{Expr, SPREAD_KEY_PREFIX, object_member},
        hooks::HookEntry,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeStrength {
    /// Dep merely versioned by `from`, or the write is conditional/imprecise.
    May,
    /// Exact-slot dep ∧ must-fresh write on all paths.
    Must,
}

#[derive(Debug, Clone)]
pub struct ChurnEdge {
    pub from: QualifiedSlot,
    pub to: QualifiedSlot,
    pub strength: EdgeStrength,
    /// Component whose effect carries this edge.
    pub component: ComponentId,
    pub effect_label: HookLabel,
    pub write_span: Option<SourceRange>,
    /// The edge exists because the carrying effect re-runs after every render
    /// (its re-run class, `engine::triggers::Rerun::Always`), and why: no
    /// dependency array, a dep that is a new reference every render, … —
    /// and whether it surely does: a fresh dep the parent allocates is new
    /// only when the parent re-renders.
    pub always: Option<(Always, bool)>,
    /// A dep-driven edge from a slot into itself: the self-churn arm's
    /// partition (ADR-020 item 2). Never part of a cycle the graph reports.
    pub self_slot: bool,
}

/// One churn cycle: indices into the edge list, in cycle order
/// (`edges[i].to == edges[i+1].from`, last wraps to first).
#[derive(Debug, Clone)]
pub struct ChurnCycle {
    pub edge_idx: Vec<usize>,
    pub all_must: bool,
    /// The cycle involves more than one component (slot owners or effect
    /// carriers) — severity is capped at Warning: cross-component must-rerun
    /// cannot be proven (prop deps are `Versioned`, never exact).
    pub cross_component: bool,
}

/// The program's churn graph: every edge plus the cycles found in it.
///
/// Whole-program data — `build` reads every component — so it is computed
/// once per program and shared by every component's rule pass
/// ([`crate::engine::ProgramRelations`]).
pub struct ChurnGraph {
    pub edges: Vec<ChurnEdge>,
    pub cycles: Vec<ChurnCycle>,
}

#[cfg(test)]
thread_local! {
    /// Test-only count of [`ChurnGraph::build`] calls on the current thread —
    /// the guard for issue #86 (built once per program, never once per
    /// component). Thread-local, so parallel tests never observe each other.
    pub(crate) static BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl ChurnGraph {
    pub fn build(result: &ProgramAnalysisResult) -> Self {
        #[cfg(test)]
        BUILDS.with(|n| n.set(n.get() + 1));
        let edges = build_edges(result);
        let cycles = if edges.is_empty() {
            Vec::new()
        } else {
            find_cycles(&edges)
        };
        ChurnGraph { edges, cycles }
    }
}

/// The slot a writer row writes, qualified by its owner.
fn node_of(component: ComponentId, w: &SlotWriter) -> QualifiedSlot {
    (w.owner.unwrap_or(component), w.slot)
}

/// One element of a component in another one's render: where it mounts.
struct Element<'a> {
    /// The component whose render builds the element, and hands it its
    /// props.
    parent: ComponentId,
    /// The element is built inside a closure (`items.map(i => <Child/>)`),
    /// which mounts it once per item the loop may add.
    closure: bool,
    /// What must hold still in the parent for the element to stay mounted:
    /// the guards of its block and its `key`.
    held: Vec<&'a Expr>,
}

/// The program's component tree, child → the elements that mount it (#213).
/// A component another one renders only through a value the lowering cannot
/// place (`origin: None`) has no element here.
type ComponentTree<'a> = HashMap<ComponentId, Vec<Element<'a>>>;

fn component_tree(result: &ProgramAnalysisResult) -> ComponentTree<'_> {
    /// Every node under `e`, with whether it sits inside a closure.
    fn walk<'a>(e: &'a Expr, closure: bool, f: &mut dyn FnMut(&'a Expr, bool)) {
        f(e, closure);
        if let Expr::FnLit { body_cfg, .. } = e {
            body_cfg.for_each_expr(&mut |x| walk(x, true, f));
        }
        e.for_each_child(&mut |c| walk(c, closure, f));
    }
    let mut tree: ComponentTree = HashMap::new();
    for (&parent, r) in &result.components {
        let cfg = &r.render_cfg;
        for block in cfg.blocks.values() {
            let guards: Vec<&Expr> = site_guards(cfg, block.id)
                .into_iter()
                .map(|(c, _)| c)
                .collect();
            block.for_each_expr(&mut |e| {
                walk(e, false, &mut |e, closure| {
                    let Expr::CompApp {
                        origin: Some(o),
                        props,
                        ..
                    } = e
                    else {
                        return;
                    };
                    let Some(child) = result.component_table.id_of(o) else {
                        return;
                    };
                    if child == parent {
                        return;
                    }
                    let mut held = guards.clone();
                    if let Expr::ObjectLit { fields, .. } = props.peel_ts()
                        && let Some(key) = object_member(fields, "key")
                    {
                        held.push(key);
                    }
                    tree.entry(child).or_default().push(Element {
                        parent,
                        closure,
                        held,
                    });
                });
            });
        }
    }
    tree
}

/// Every component whose render builds `c`'s element or an ancestor's: the
/// components whose re-render may hand `c` new props.
fn ancestors(c: ComponentId, tree: &ComponentTree<'_>) -> HashSet<ComponentId> {
    let mut seen: HashSet<ComponentId> = HashSet::new();
    let mut stack = vec![c];
    while let Some(x) = stack.pop() {
        for e in tree.get(&x).into_iter().flatten() {
            if seen.insert(e.parent) {
                stack.push(e.parent);
            }
        }
    }
    seen
}

/// Build all churn edges of the program.
pub fn build_edges(result: &ProgramAnalysisResult) -> Vec<ChurnEdge> {
    // Per-component facts the proofs read.
    struct CompCtx<'a> {
        /// The names some render binding ties to a slot's read: a may
        /// reading, for what may move.
        state_vals: HashMap<Var, HookLabel>,
        /// The names the render binds to a slot's read on every path, no
        /// other body rebinding them (`setters::must_state_vals`): what the
        /// proofs may read as the slot itself (#210, #222).
        must_state_vals: HashMap<Var, HookLabel>,
        memo_vals: HashMap<Var, HookLabel>,
        memo_deps: HashMap<HookLabel, &'a [Expr]>,
        render_lets: HashMap<&'a str, Option<&'a Expr>>,
        mutated: HashSet<Var>,
        /// The props hold across every loop the component's edges can be on:
        /// no write the loop can run moves a slot of an ancestor (#206), and
        /// no effect of the component reacts to another component's slot.
        /// Otherwise the props may move on any cycle through the component —
        /// the proofs then read no fact over them. An effect with no deps
        /// array has no trigger row, so the rows alone cannot say it.
        props_hold: bool,
        /// The render exit env: what the proofs narrow from.
        exit: AbstractEnv<StateValue>,
    }
    // Per-effect facts, gathered first so the write sites of every slot are
    // known program-wide before any convergence kill is attempted (see
    // module doc).
    struct EffectFacts<'a> {
        comp: ComponentId,
        comp_result: &'a AnalysisResult<StateValue>,
        body_cfg: &'a CFG,
        effect_label: HookLabel,
        /// The effect re-runs after every render, and whether that is
        /// certain: what its no-dependency-array path reads.
        always: Option<(Always, bool)>,
        deps: &'a [Expr],
        triggers: Vec<&'a EffectTrigger>,
        exact_local: HashSet<HookLabel>,
        versioned: HashSet<QualifiedSlot>,
        /// A dep may move with the props: `P_c` is a trigger (#207).
        props: bool,
        writes: Vec<&'a SlotWriter>,
    }
    /// One write site of a slot, with the body it sits in (module doc,
    /// "What is a site").
    struct SiteRef<'a> {
        comp: ComponentId,
        cfg: &'a CFG,
        row: &'a SlotWriter,
    }

    let mut all_sites: Vec<SiteRef> = Vec::new();
    let mut ctxs: HashMap<ComponentId, CompCtx> = HashMap::new();
    let mut facts: Vec<EffectFacts> = Vec::new();

    // A module name any component writes moves between two runs of any
    // component's effect: the union is program-wide, as `render_deps` reads
    // it.
    let module_written: HashSet<Var> = result.components.values().flat_map(written_names).collect();
    // A visible navigation in any body the loop can run moves every
    // navigation-held value program-wide (#161).
    let navigates = result.components.values().any(|c| {
        let render = local_bindings(&c.render_cfg);
        navigates(&c.render_cfg, &render)
            || c.hooks
                .iter()
                .filter(|h| !matches!(h, HookEntry::Handler { .. }))
                .filter_map(HookEntry::body_cfg)
                .any(|b| navigates(b, &render))
    });

    // The component tree (#213), and the components a write the loop can run
    // may move a slot of: a write through a callback prop included, so
    // wherever it sits — the child itself, a sibling subtree (#206).
    let tree = component_tree(result);
    let moved: HashSet<QualifiedSlot> = result
        .components
        .iter()
        .flat_map(|(&c, r)| {
            r.slot_writers
                .iter()
                .filter(|w| w.phase != WriterPhase::Handler)
                .map(move |w| node_of(c, w))
        })
        .collect();
    // The props node `P_c` (#207), by its in-edges: every slot of an
    // ancestor some write the loop can run moves — `P_parent → P_c` is
    // folded in, the ancestors being transitive. An edge out of `P_c` is
    // pushed from each of them, so every edge stays a slot-to-slot write
    // edge with its carrying effect.
    let props_in: HashMap<ComponentId, Vec<QualifiedSlot>> = result
        .components
        .keys()
        .map(|&c| {
            let up = ancestors(c, &tree);
            let mut v: Vec<QualifiedSlot> = moved
                .iter()
                .filter(|s| up.contains(&s.0))
                .copied()
                .collect();
            v.sort();
            (c, v)
        })
        .collect();

    for (&comp, comp_result) in &result.components {
        let cfg = &comp_result.render_cfg;
        let mut mutated = module_written.clone();
        mutated.extend(mutated_roots(cfg));
        for body in comp_result.hooks.iter().filter_map(HookEntry::body_cfg) {
            mutated.extend(mutated_roots(body));
        }
        let state_vals = resolve_setter_aliases(cfg, &state_val_labels(cfg));
        let must_state_vals = must_state_vals(cfg, &must_alias_scopes(cfg, &comp_result.hooks));
        ctxs.insert(
            comp,
            CompCtx {
                state_vals,
                must_state_vals,
                memo_vals: resolve_setter_aliases(cfg, &memo_val_labels(cfg)),
                memo_deps: comp_result
                    .hooks
                    .iter()
                    .filter_map(|h| match h {
                        HookEntry::Memo { label, deps, .. }
                        | HookEntry::Callback { label, deps, .. } => deps
                            .list()
                            .filter(|d| d.arity.exact().is_some())
                            .map(|d| (*label, d.as_slice())),
                        _ => None,
                    })
                    .collect(),
                render_lets: let_bindings(cfg),
                mutated,
                props_hold: comp_result.effect_triggers.iter().all(|t| t.slot.0 == comp)
                    && props_in[&comp].is_empty(),
                exit: comp_result.exit_env(),
            },
        );
    }

    // Which components stay mounted across the loop, on the component tree
    // (#162, #213).
    let mut stays_memo: HashMap<ComponentId, bool> = HashMap::new();
    let kept_mounted: HashMap<ComponentId, bool> = result
        .components
        .keys()
        .map(|&c| {
            (
                c,
                stays(c, &tree, result, &ctxs, navigates, &mut stays_memo),
            )
        })
        .collect();

    for (&comp, comp_result) in &result.components {
        let cfg = &comp_result.render_cfg;
        let hook = |label: HookLabel| comp_result.hooks.iter().find(|h| h.label() == label);
        // Mount-only effects fire once for a component that stays mounted —
        // but only an array the engine knows is empty says so.
        let mount_only = |label: HookLabel| comp_result.rerun(label) == Some(Rerun::MountOnly);
        for w in comp_result
            .slot_writers
            .iter()
            .filter(|w| w.phase != WriterPhase::Handler)
        {
            let body = match w.region {
                WriterRegion::Render => Some(cfg),
                WriterRegion::Effect(l) if mount_only(l) => match w.owner {
                    // The component itself stays mounted and fires it once.
                    None => None,
                    // A child the loop mounts and unmounts fires it every
                    // round.
                    Some(_) if kept_mounted[&comp] => None,
                    Some(_) => hook(l).and_then(HookEntry::body_cfg),
                },
                WriterRegion::Effect(l) | WriterRegion::Memo(l) => {
                    hook(l).and_then(HookEntry::body_cfg)
                }
                // A callback's writes are rows of the body that calls it.
                WriterRegion::Callback(_) | WriterRegion::Handler(_) => None,
            };
            if let Some(body) = body {
                all_sites.push(SiteRef {
                    comp,
                    cfg: body,
                    row: w,
                });
            }
        }
        for hook in &comp_result.hooks {
            let HookEntry::Effect {
                label,
                body_cfg,
                deps,
                ..
            } = hook
            else {
                continue;
            };
            if mount_only(*label) {
                continue;
            }
            // Every non-handler write row of the body, any freshness: a
            // `setX(null)` still revives guards, so it must block convergence
            // kills on X.
            let writes: Vec<&SlotWriter> = comp_result
                .slot_writers
                .iter()
                .filter(|w| w.region == WriterRegion::Effect(*label))
                .filter(|w| w.phase != WriterPhase::Handler)
                .collect();
            if writes.is_empty() {
                continue;
            }
            let triggers: Vec<&EffectTrigger> =
                triggers_of(&comp_result.effect_triggers, *label).collect();
            let mut exact_local: HashSet<HookLabel> = HashSet::new();
            let mut versioned: HashSet<QualifiedSlot> = HashSet::new();
            for t in &triggers {
                if t.exact {
                    exact_local.insert(t.slot.1);
                } else {
                    versioned.insert(t.slot);
                }
            }
            let deps = deps.list().map_or(&[][..], |d| d.as_slice());
            // An effect that re-runs after every render is read the way one
            // with no deps array is. Certain when it has none, or when a dep
            // is a new reference at every render — the same reading of
            // "fresh" as a write's (`restorable` below): a reference that
            // holds while props hold is a prop the parent allocated, the same
            // when the component re-renders alone. An unreadable deps
            // argument or a ref's contents only may re-run it (#212, #201).
            let always = comp_result.rerun(*label).and_then(Rerun::always).map(|a| {
                let certain = match a {
                    Always::FreshDep(i) => deps.get(i).is_some_and(|d| {
                        !invariance_of(&ctxs[&comp], navigates).holds(
                            d,
                            &ctxs[&comp].render_lets,
                            true,
                        )
                    }),
                    _ => a.certain(),
                };
                (a, certain)
            });
            facts.push(EffectFacts {
                comp,
                comp_result,
                body_cfg,
                effect_label: *label,
                always,
                deps,
                triggers,
                exact_local,
                versioned,
                props: comp_result.effect_props.contains(label),
                writes,
            });
        }
    }

    // A slot some other component also writes: its guards live in bodies
    // this component's env cannot read, so it is never killed.
    let foreign: HashSet<QualifiedSlot> = all_sites
        .iter()
        .map(|s| node_of(s.comp, s.row))
        .zip(all_sites.iter())
        .filter(|(node, s)| node.0 != s.comp)
        .map(|(node, _)| node)
        .collect();
    let by_comp: HashMap<ComponentId, Vec<usize>> =
        all_sites
            .iter()
            .enumerate()
            .fold(HashMap::new(), |mut m, (i, s)| {
                m.entry(s.comp).or_default().push(i);
                m
            });
    fn invariance_of<'c>(ctx: &'c CompCtx<'_>, navigates: bool) -> Invariance<'c> {
        Invariance {
            render: &ctx.render_lets,
            state_vals: &ctx.state_vals,
            memo_vals: &ctx.memo_vals,
            memo_deps: &ctx.memo_deps,
            mutated: &ctx.mutated,
            navigates,
        }
    }
    /// Does every element that mounts `c` keep it mounted across the loop,
    /// and is its parent kept mounted in turn (#213)? An element keeps its
    /// child when it sits outside any closure, under guards that hold still
    /// in the parent, with no `key` or one that holds. A component no
    /// element mounts is a root and stays; one the table cannot place, or
    /// one met again while its own answer is pending (a component that
    /// renders an ancestor of itself), fails closed.
    fn stays(
        c: ComponentId,
        tree: &ComponentTree<'_>,
        result: &ProgramAnalysisResult,
        ctxs: &HashMap<ComponentId, CompCtx<'_>>,
        navigates: bool,
        memo: &mut HashMap<ComponentId, bool>,
    ) -> bool {
        if let Some(&s) = memo.get(&c) {
            return s;
        }
        memo.insert(c, false);
        let s = result.component_table.origin(c).is_some()
            && tree.get(&c).is_none_or(|els| {
                els.iter().all(|e| {
                    let ctx = &ctxs[&e.parent];
                    let invariance = invariance_of(ctx, navigates);
                    !e.closure
                        && e.held
                            .iter()
                            .all(|x| invariance.holds(x, &ctx.render_lets, ctx.props_hold))
                        && stays(e.parent, tree, result, ctxs, navigates, memo)
                })
            });
        memo.insert(c, s);
        s
    }
    fn site_of<'a>(s: &SiteRef<'a>, convergent: bool) -> WriteSite<'a> {
        WriteSite {
            component: s.comp,
            cfg: s.cfg,
            slot: node_of(s.comp, s.row),
            guard_block: s.row.guard_block,
            block: s.row.block,
            bounded: s.row.phase != WriterPhase::Unknown,
            convergent,
            value: &s.row.written.value,
            expr: s.row.written.expr.as_ref(),
        }
    }
    /// Does every write of `slot` the automatic loop runs move it one way by
    /// a constant (`s + 1`, `s - 2`, `p => p + 1`), and store a value in a
    /// finite interval where it runs? A strictly monotone sequence in a
    /// bounded set ends, so the slot settles whatever its guards say. A write
    /// from another component is not read: its slot read is a prop.
    fn moves_one_way(
        slot: QualifiedSlot,
        sites: &[SiteRef<'_>],
        ctxs: &HashMap<ComponentId, CompCtx<'_>>,
    ) -> bool {
        // Bounded: what the write stores, read where it runs (under its
        // guards), is a finite interval, and no kind `+` could grow instead
        // (a string concatenates, a reference or an opaque value is
        // anything). Every value after the first write is one of those.
        let bounded = |v: &StateValue| {
            v.num.lo.is_finite()
                && v.num.hi.is_finite()
                && v.str == crate::domains::impls::StrConst::Bottom
                && v.reference == crate::domains::Stability::Bottom
                && !v.other
        };
        let mut way = None;
        for s in sites.iter().filter(|s| node_of(s.comp, s.row) == slot) {
            if !bounded(&s.row.written.value) {
                return false;
            }
            let Some(ctx) = ctxs.get(&s.comp) else {
                return false;
            };
            // A name reads the slot when the render binds it to the read on
            // every path and the write's own body does not rebind it.
            let local = (s.row.region != WriterRegion::Render).then(|| let_bindings(s.cfg));
            let reads_slot = |v: &Var| {
                ctx.must_state_vals.get(v) == Some(&slot.1)
                    && !local.as_ref().is_some_and(|b| b.contains_key(v.as_str()))
            };
            let step = match (s.row.owner, s.row.written.expr.as_ref()) {
                (None, Some(e)) => step_of(e, slot.1, &reads_slot),
                _ => None,
            };
            match (step, way) {
                (None, _) => return false,
                (Some(d), None) => way = Some(d),
                (Some(d), Some(w)) if d != w => return false,
                _ => {}
            }
        }
        return way.is_some();

        /// `+1` / `-1` when `e` adds to / subtracts from `label`'s read a
        /// positive constant, directly or as an updater's only return.
        fn step_of(e: &Expr, label: HookLabel, reads_slot: &dyn Fn(&Var) -> bool) -> Option<i8> {
            let positive = |e: &Expr| match e.peel_ts() {
                Expr::Lit(crate::ir::expr::Prim::Int(n)) => *n > 0,
                Expr::Lit(crate::ir::expr::Prim::Float(f)) => f.is_finite() && *f > 0.0,
                _ => false,
            };
            let step = |e: &Expr, is_read: &dyn Fn(&Expr) -> bool| match e.peel_ts() {
                Expr::BinOp {
                    op: crate::ir::expr::BinOp::Add,
                    lhs,
                    rhs,
                } if (is_read(lhs) && positive(rhs)) || (positive(lhs) && is_read(rhs)) => Some(1),
                Expr::BinOp {
                    op: crate::ir::expr::BinOp::Sub,
                    lhs,
                    rhs,
                } if is_read(lhs) && positive(rhs) => Some(-1),
                _ => None,
            };
            match e.peel_ts() {
                Expr::FnLit {
                    params, body_cfg, ..
                } => {
                    let [p] = params.as_slice() else {
                        return None;
                    };
                    let mut returns = body_cfg.blocks.values().filter_map(|b| match &b.term {
                        Terminator::Return(r) => Some(r),
                        _ => None,
                    });
                    let (Some(r), None) = (returns.next(), returns.next()) else {
                        return None;
                    };
                    step(r, &|x| matches!(x.peel_ts(), Expr::Var(v) if v == p))
                }
                other => step(other, &|x| match x.peel_ts() {
                    Expr::StateVal(l) => *l == label,
                    Expr::Var(v) => reads_slot(v),
                    _ => false,
                }),
            }
        }
    }
    /// Can the loop re-run the body `s` sits in? A render or memo body, an
    /// effect that re-runs after every render, one with a slot among its
    /// triggers or with props a loop moves (#207), or a mount-only effect of
    /// a component the loop may remount. An effect whose deps no slot moves
    /// re-runs only when something outside the loop changes.
    fn reruns_in_loop(
        s: &SiteRef<'_>,
        result: &ProgramAnalysisResult,
        props_in: &HashMap<ComponentId, Vec<QualifiedSlot>>,
        kept_mounted: &HashMap<ComponentId, bool>,
    ) -> bool {
        let WriterRegion::Effect(l) = s.row.region else {
            return true;
        };
        let Some(r) = result.components.get(&s.comp) else {
            return true;
        };
        match r.rerun(l) {
            Some(Rerun::Always(_)) => true,
            Some(Rerun::MountOnly) => kept_mounted.get(&s.comp) != Some(&true),
            Some(Rerun::OnSlots) | None => {
                triggers_of(&r.effect_triggers, l).next().is_some()
                    || (r.effect_props.contains(&l)
                        && props_in.get(&s.comp).is_none_or(|p| !p.is_empty()))
            }
        }
    }
    let invariance_of = |ctx| invariance_of(ctx, navigates);

    // The sites that fire at most once in the automatic loop, by least
    // fixpoint (module doc). Monotone — a proven site only removes
    // revivers from the others — so the result does not depend on the
    // order the components are visited in.
    let mut convergent = vec![false; all_sites.len()];
    loop {
        let mut changed = false;
        for (comp, idxs) in &by_comp {
            let ctx = &ctxs[comp];
            let invariance = invariance_of(ctx);
            let mut evaluator = result.components[comp].evaluator();
            let mut eval = |e: &Expr| evaluator.at(&ctx.exit, e);
            let peers: Vec<WriteSite> = idxs
                .iter()
                .map(|&j| site_of(&all_sites[j], convergent[j]))
                .collect();
            for (k, &j) in idxs.iter().enumerate() {
                if convergent[j] || all_sites[j].row.owner.is_some() {
                    continue;
                }
                if converges_under_all_writes(
                    &peers,
                    k,
                    peers[k].value,
                    &foreign,
                    &ctx.must_state_vals,
                    &invariance,
                    ctx.props_hold,
                    &ctx.exit,
                    &mut eval,
                ) {
                    convergent[j] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Deduplicate on (from, to, component, effect): keep the strongest, and
    // among equals the earliest write site, so the row a reader anchors on
    // does not depend on relation order.
    let mut best: HashMap<(QualifiedSlot, QualifiedSlot, ComponentId, HookLabel), ChurnEdge> =
        HashMap::new();
    let mut one_way: HashMap<QualifiedSlot, bool> = HashMap::new();
    // The slots some write may restore within a batch (#188): React re-renders
    // on the batch's last value, and nothing here orders the writes of one
    // batch — two effects, a layout effect and the passive effects flushed
    // before its re-render, a mount-only effect, a callback prop a child
    // calls, which is a `Handler` row — so a fresh write proves a change only
    // when every write of its slot is fresh. Fresh at every call means a new
    // reference (`Change::Must`) that does not hold while props hold: a prop
    // the parent allocates is new at the parent's renders, and `setS(p)`
    // stores the same `p` again when the component re-renders alone.
    let mut restorable: HashSet<QualifiedSlot> = HashSet::new();
    for (&comp, r) in &result.components {
        let invariance = invariance_of(&ctxs[&comp]);
        for w in &r.slot_writers {
            let body = match w.region {
                WriterRegion::Render => Some(&r.render_cfg),
                WriterRegion::Effect(l)
                | WriterRegion::Memo(l)
                | WriterRegion::Callback(l)
                | WriterRegion::Handler(l) => r
                    .hooks
                    .iter()
                    .find(|h| h.label() == l)
                    .and_then(HookEntry::body_cfg),
            };
            let fresh = w.written.change == Change::Must
                && body.is_some_and(|b| {
                    !w.written
                        .expr
                        .as_ref()
                        .is_some_and(|e| invariance.holds(e, &let_bindings(b), true))
                });
            if !fresh {
                restorable.insert(node_of(comp, w));
            }
        }
    }
    for f in &facts {
        let ctx = &ctxs[&f.comp];
        let invariance = invariance_of(ctx);
        let mut evaluator = f.comp_result.evaluator();
        let mut eval = |e: &Expr| evaluator.at(&ctx.exit, e);
        let idxs = by_comp.get(&f.comp).map_or(&[][..], Vec::as_slice);
        let peers: Vec<WriteSite> = idxs
            .iter()
            .map(|&j| site_of(&all_sites[j], convergent[j]))
            .collect();
        for w in &f.writes {
            if w.written.change == Change::Never {
                continue;
            }
            let node = node_of(f.comp, w);
            // Can another write the loop runs move the slot away from what
            // this one leaves in it, again and again? An identity cannot, nor
            // a write of the same single value this one stores, nor a site
            // proven to fire at most once, nor an effect nothing in the loop
            // re-runs — no slot among its triggers, the graph's own reading:
            // such an effect has no edge into it either.
            let moved = all_sites
                .iter()
                .enumerate()
                .filter(|(j, s)| {
                    node_of(s.comp, s.row) == node
                        && !std::ptr::eq(s.row, *w)
                        && !convergent[*j]
                        && reruns_in_loop(s, result, &props_in, &kept_mounted)
                })
                .any(|(_, s)| match s.row.written.change {
                    Change::Never => false,
                    Change::Once => {
                        w.written.change != Change::Once || s.row.written.value != w.written.value
                    }
                    Change::May | Change::Must => true,
                });
            // One value known in advance changes the slot once, unless
            // another write keeps moving it away.
            if w.written.change == Change::Once && !moved {
                continue;
            }
            // Convergence proof: once a written value sits in the slot, do the
            // dominating guards kill this write — under its own write and
            // under every other site's? Edges claim churn, so the own write is
            // read as the part of its value that churns: its reference
            // (truthy and non-nullish) and its unbounded primitives; another
            // site's write revives with whatever it stores, `null` included.
            // A single value another write moves away is stored again whole.
            let own_value = if w.written.change == Change::Once {
                w.written.value.clone()
            } else {
                churning_part(&w.written.value)
            };
            let k = idxs
                .iter()
                .position(|&j| std::ptr::eq(all_sites[j].row, *w))
                .expect("every effect write is a site");
            let killed = convergent[idxs[k]]
                || *one_way
                    .entry(node)
                    .or_insert_with(|| moves_one_way(node, &all_sites, &ctxs))
                || converges_under_all_writes(
                    &peers,
                    k,
                    &own_value,
                    &foreign,
                    &ctx.must_state_vals,
                    &invariance,
                    ctx.props_hold,
                    &ctx.exit,
                    &mut eval,
                );
            // What the write may depend on: it changes the slot only when one
            // of those moved since it last ran. `None` — a new reference, an
            // updater, an opaque value — may change it whatever moved, and so
            // does a write that reads its own slot, which its last run moved,
            // or a slot another write moves away.
            let reads = match (&w.written.expr, w.written.change) {
                _ if moved => None,
                (Some(e), Change::May) => {
                    let r = &f.comp_result;
                    invariance.reads(e, f.body_cfg, &r.render_cfg, &|v| {
                        v == r.param || r.module_consts.contains_key(v)
                    })
                }
                _ => None,
            }
            .filter(|r| !(node.0 == f.comp && r.slots.contains(&node.1)))
            // A call over the reads is a function of them only when it
            // cannot hand back a new reference (#218).
            .filter(|r| !r.call || !may_allocate(&w.written.value, r, f.comp));
            let reads_slot = |x: QualifiedSlot| match &reads {
                None => true,
                Some(r) if x.0 == f.comp => r.slots.contains(&x.1),
                Some(r) => r.props,
            };
            let fresh_blocks: HashSet<BlockId> = f
                .writes
                .iter()
                .filter(|o| node_of(f.comp, o) == node && o.written.change == Change::Must)
                .filter_map(|o| o.block)
                .collect();
            let must_write = w.written.change == Change::Must
                && w.block.is_some()
                && on_all_paths(f.body_cfg, &fresh_blocks)
                && !restorable.contains(&node)
                // The re-render a loop needs happens only if its component
                // is still mounted: an ancestor whose guard or key moves may
                // unmount it in the very batch that should re-run it.
                && kept_mounted[&f.comp];
            let strength = if must_write {
                EdgeStrength::Must
            } else {
                EdgeStrength::May
            };

            let mut push = |from: QualifiedSlot,
                            strength: EdgeStrength,
                            self_slot: bool,
                            always: Option<(Always, bool)>| {
                insert_edge(
                    &mut best,
                    ChurnEdge {
                        from,
                        to: node,
                        strength,
                        component: f.comp,
                        effect_label: f.effect_label,
                        write_span: w.span,
                        always,
                        self_slot,
                    },
                );
            };

            if killed {
                continue;
            }
            // The props node's edges into this write (#207): a write that
            // reads a prop, or anything, changes when `P_c` moves. Where no
            // edge enters `P_c` — a root, or a component rendered through a
            // value the tree cannot place — the write's own slot stands in
            // for it, a self-edge, as before the node existed.
            let p_in = &props_in[&f.comp];
            let reads_props = reads.as_ref().is_none_or(|r| r.props);
            let from_props: &[QualifiedSlot] = if reads_props { p_in } else { &[] };
            // An effect that re-runs after every render: a change of anything
            // the write reads re-runs it (a handler row was dropped above), so
            // each slot it reads is an edge into its own, and a write that
            // reads everything re-triggers itself, whichever turn it runs on.
            let every_render: Vec<QualifiedSlot> = match &reads {
                Some(r) => {
                    let props = (r.props && p_in.is_empty()).then_some(node);
                    r.slots.iter().map(|&l| (f.comp, l)).chain(props).collect()
                }
                None => vec![node],
            };
            // A certain re-run reads every row already.
            if let Some((_, true)) = f.always {
                for &x in &every_render {
                    push(x, strength, false, f.always);
                }
                for &x in from_props {
                    push(x, EdgeStrength::May, false, f.always);
                }
                continue;
            }
            let retrigger = || write_can_retrigger(f, &ctx.must_state_vals, w);
            for &l in &f.exact_local {
                let x: QualifiedSlot = (f.comp, l);
                let self_slot = x == node;
                let dropped = !reads_slot(x) || (self_slot && !retrigger());
                if !dropped {
                    push(x, strength, self_slot, None);
                }
            }
            for &x in &f.versioned {
                if f.exact_local.contains(&x.1) && x.0 == f.comp {
                    continue; // already pushed as exact
                }
                let self_slot = x == node && x.0 == f.comp;
                let dropped = !reads_slot(x) || (self_slot && !retrigger());
                if !dropped {
                    push(x, EdgeStrength::May, self_slot, None);
                }
            }
            // A dep the props move: `P_c` is a trigger.
            if f.props {
                for &x in from_props {
                    push(x, EdgeStrength::May, false, None);
                }
            }
            // A re-run that is only possible adds May edges beside the rows'
            // own, which keep their partition on a tie (pushed first).
            if let Some((_, false)) = f.always {
                for &x in every_render.iter().chain(from_props) {
                    push(x, EdgeStrength::May, false, f.always);
                }
            }
            // The reads-kill says a re-run stores what the last run stored,
            // which holds only if nothing the write reads moved in between. A
            // read that is no trigger moves without re-running the effect,
            // and the next re-run a trigger causes stores its new value: the
            // write's edges come from what it reads, trigger or not (#204),
            // the props node included.
            if let Some(r) = &reads
                && (!f.triggers.is_empty() || f.props)
            {
                for &l in &r.slots {
                    push((f.comp, l), EdgeStrength::May, false, None);
                }
                for &x in from_props {
                    push(x, EdgeStrength::May, false, None);
                }
            }
        }
    }

    // Mount edges `P_c → y` (#207): a component the loop may remount runs
    // every effect again at each mount, its mount-only ones included, and
    // its fresh instance's writes are bounded by no certificate of the old
    // one — only an identity changes nothing.
    for (&comp, r) in &result.components {
        if kept_mounted[&comp] {
            continue;
        }
        for w in r.slot_writers.iter().filter(|w| {
            matches!(w.region, WriterRegion::Effect(_))
                && w.phase != WriterPhase::Handler
                && w.written.change != Change::Never
        }) {
            let WriterRegion::Effect(effect_label) = w.region else {
                continue;
            };
            for &from in &props_in[&comp] {
                insert_edge(
                    &mut best,
                    ChurnEdge {
                        from,
                        to: node_of(comp, w),
                        strength: EdgeStrength::May,
                        component: comp,
                        effect_label,
                        write_span: w.span,
                        always: None,
                        self_slot: false,
                    },
                );
            }
        }
    }

    let mut edges: Vec<ChurnEdge> = best.into_values().collect();
    edges.sort_by(|a, b| {
        (&a.component, a.effect_label, &a.from, &a.to).cmp(&(
            &b.component,
            b.effect_label,
            &b.from,
            &b.to,
        ))
    });
    return edges;

    /// A position to rank spans by; a spanless site ranks last.
    fn pos(s: Option<SourceRange>) -> (u32, u32) {
        s.map_or((u32::MAX, u32::MAX), |r| r.pos_key())
    }

    /// Keep, per (from, to, component, effect), the strongest edge, and
    /// among equals the earliest write site.
    fn insert_edge(
        best: &mut HashMap<(QualifiedSlot, QualifiedSlot, ComponentId, HookLabel), ChurnEdge>,
        edge: ChurnEdge,
    ) {
        let key = (edge.from, edge.to, edge.component, edge.effect_label);
        best.entry(key)
            .and_modify(|e| {
                if (edge.strength, std::cmp::Reverse(pos(edge.write_span)))
                    > (e.strength, std::cmp::Reverse(pos(e.write_span)))
                {
                    *e = edge.clone();
                }
            })
            .or_insert(edge);
    }

    /// May a written value that passed through a call be a reference the
    /// call allocated? Not when it holds no reference, one stable reference,
    /// or one only the write's reads move; a residual kind may be a fresh
    /// symbol (#218).
    fn may_allocate(v: &StateValue, r: &Reads, comp: ComponentId) -> bool {
        v.other
            || match &v.reference {
                Stability::Bottom | Stability::Stable => false,
                Stability::Versioned(s) => !s.iter().all(|&(c, l)| {
                    if c == comp {
                        r.slots.contains(&l)
                    } else {
                        r.props
                    }
                }),
                Stability::VersionedTop | Stability::PerRender | Stability::Unknown => true,
            }
    }

    /// Can a write of `w` into its own slot change any dep of this effect?
    /// (#90)
    fn write_can_retrigger(
        f: &EffectFacts<'_>,
        state_vals: &HashMap<Var, HookLabel>,
        w: &SlotWriter,
    ) -> bool {
        can_retrigger(
            f.deps,
            &f.triggers,
            f.comp,
            w.slot,
            state_vals,
            w.written.expr.as_ref(),
        )
    }
}

/// Find churn cycles: first in the must-only subgraph (all-must cycles),
/// then in the full graph, skipping regions already reported. Self-slot
/// edges are the other arm's and never enter the search.
pub fn find_cycles(edges: &[ChurnEdge]) -> Vec<ChurnCycle> {
    let must_idx: Vec<usize> = (0..edges.len())
        .filter(|&i| !edges[i].self_slot && edges[i].strength == EdgeStrength::Must)
        .collect();
    let all_idx: Vec<usize> = (0..edges.len()).filter(|&i| !edges[i].self_slot).collect();

    let mut cycles = Vec::new();
    let mut covered_nodes: HashSet<QualifiedSlot> = HashSet::new();

    for cyc in cycles_in(edges, &must_idx) {
        for &i in &cyc {
            covered_nodes.insert(edges[i].from);
            covered_nodes.insert(edges[i].to);
        }
        cycles.push(make_cycle(edges, cyc, true));
    }
    for cyc in cycles_in(edges, &all_idx) {
        // A node already inside an all-must cycle: the Error already flags
        // that loop region — don't re-report a weaker overlapping cycle.
        if cyc.iter().any(|&i| {
            covered_nodes.contains(&edges[i].from) || covered_nodes.contains(&edges[i].to)
        }) {
            continue;
        }
        cycles.push(make_cycle(edges, cyc, false));
    }
    cycles
}

fn make_cycle(edges: &[ChurnEdge], edge_idx: Vec<usize>, all_must: bool) -> ChurnCycle {
    let mut comps: HashSet<ComponentId> = HashSet::new();
    for &i in &edge_idx {
        comps.insert(edges[i].from.0);
        comps.insert(edges[i].to.0);
        comps.insert(edges[i].component);
    }
    ChurnCycle {
        all_must,
        cross_component: comps.len() > 1,
        edge_idx,
    }
}

/// One simple cycle per cyclic SCC of the subgraph `subset`, as edge indices
/// in cycle order.
fn cycles_in(edges: &[ChurnEdge], subset: &[usize]) -> Vec<Vec<usize>> {
    // Node table (sorted for determinism).
    let mut nodes: Vec<&QualifiedSlot> = subset
        .iter()
        .flat_map(|&i| [&edges[i].from, &edges[i].to])
        .collect();
    nodes.sort();
    nodes.dedup();
    let node_id: HashMap<&QualifiedSlot, usize> =
        nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()]; // node → edge indices
    for &e in subset {
        adj[node_id[&edges[e].from]].push(e);
    }

    let sccs = tarjan_sccs(nodes.len(), &adj, edges, &node_id);

    let mut out = Vec::new();
    for scc in sccs {
        let scc_set: HashSet<usize> = scc.iter().copied().collect();
        let is_cyclic = scc.len() >= 2
            || scc
                .iter()
                .any(|&n| adj[n].iter().any(|&e| node_id[&edges[e].to] == n));
        if !is_cyclic {
            continue;
        }
        let start = *scc.iter().min().unwrap();
        if let Some(cycle) = find_cycle_from(start, &adj, &scc_set, edges, &node_id) {
            out.push(cycle);
        }
    }
    out
}

/// Tarjan strongly-connected components (recursive; graphs are tiny).
fn tarjan_sccs(
    n: usize,
    adj: &[Vec<usize>],
    edges: &[ChurnEdge],
    node_id: &HashMap<&QualifiedSlot, usize>,
) -> Vec<Vec<usize>> {
    struct St<'a> {
        adj: &'a [Vec<usize>],
        edges: &'a [ChurnEdge],
        node_id: &'a HashMap<&'a QualifiedSlot, usize>,
        index: usize,
        indices: Vec<Option<usize>>,
        lowlink: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        sccs: Vec<Vec<usize>>,
    }
    fn strongconnect(v: usize, st: &mut St) {
        st.indices[v] = Some(st.index);
        st.lowlink[v] = st.index;
        st.index += 1;
        st.stack.push(v);
        st.on_stack[v] = true;
        for &e in &st.adj[v] {
            let w = st.node_id[&st.edges[e].to];
            if st.indices[w].is_none() {
                strongconnect(w, st);
                st.lowlink[v] = st.lowlink[v].min(st.lowlink[w]);
            } else if st.on_stack[w] {
                st.lowlink[v] = st.lowlink[v].min(st.indices[w].unwrap());
            }
        }
        if st.lowlink[v] == st.indices[v].unwrap() {
            let mut scc = Vec::new();
            loop {
                let w = st.stack.pop().unwrap();
                st.on_stack[w] = false;
                scc.push(w);
                if w == v {
                    break;
                }
            }
            scc.sort_unstable();
            st.sccs.push(scc);
        }
    }
    let mut st = St {
        adj,
        edges,
        node_id,
        index: 0,
        indices: vec![None; n],
        lowlink: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        sccs: Vec::new(),
    };
    for v in 0..n {
        if st.indices[v].is_none() {
            strongconnect(v, &mut st);
        }
    }
    st.sccs
}

/// DFS inside `scc` from `start`, returning the edge path of one simple
/// cycle `start → … → start`. Exists whenever the SCC is cyclic.
fn find_cycle_from(
    start: usize,
    adj: &[Vec<usize>],
    scc: &HashSet<usize>,
    edges: &[ChurnEdge],
    node_id: &HashMap<&QualifiedSlot, usize>,
) -> Option<Vec<usize>> {
    // Iterative DFS with an explicit edge path.
    let mut path: Vec<usize> = Vec::new(); // edge indices
    let mut iters: Vec<std::slice::Iter<usize>> = vec![adj[start].iter()];
    let mut visited: HashSet<usize> = HashSet::from([start]);
    while let Some(it) = iters.last_mut() {
        match it.next() {
            Some(&e) => {
                let w = node_id[&edges[e].to];
                if w == start {
                    path.push(e);
                    return Some(path);
                }
                if scc.contains(&w) && visited.insert(w) {
                    path.push(e);
                    iters.push(adj[w].iter());
                }
            }
            None => {
                iters.pop();
                path.pop();
            }
        }
    }
    None
}

// ── Field-sensitive re-trigger (#90) ─────────────────────────────────────────

/// Can a write of `written_expr` into `label` change any dep of this effect?
///
/// A functional update that spreads its own parameter — `prev => ({ ...prev,
/// slug: f(prev) })` — stores `prev`'s value at every member the literal does
/// not name, so a dep that reads only those members is `Object.is`-equal after
/// the write and cannot re-trigger the effect. Sound by default: a dep this
/// walk cannot place under a preserved member answers `true`.
///
/// `triggers` are the effect's rows of the trigger relation: a dep reacts to
/// `label` when one of its rows names this component's slot.
fn can_retrigger(
    dep_exprs: &[Expr],
    triggers: &[&EffectTrigger],
    component: ComponentId,
    label: HookLabel,
    state_vals: &HashMap<Var, HookLabel>,
    written_expr: Option<&Expr>,
) -> bool {
    let Some(overwritten) = updater_overwrites(written_expr) else {
        return true;
    };
    for (i, dep) in dep_exprs.iter().enumerate() {
        let reacts = triggers
            .iter()
            .any(|t| t.dep == i && t.slot == (component, label));
        if !reacts {
            continue;
        }
        match slot_member(dep, label, state_vals) {
            Some(m) if !overwritten.contains(&m) => {}
            _ => return true,
        }
    }
    false
}

/// The members a functional update names explicitly, when it provably leaves
/// every other member of its parameter untouched. `None` proves nothing.
fn updater_overwrites(written_expr: Option<&Expr>) -> Option<HashSet<Symbol>> {
    let Some(Expr::FnLit {
        params, body_cfg, ..
    }) = written_expr.map(Expr::peel_ts)
    else {
        return None;
    };
    let [prev] = params.as_slice() else {
        return None;
    };
    let mut out = HashSet::new();
    let mut returns = 0usize;
    for block in body_cfg.blocks.values() {
        let crate::ir::cfg::Terminator::Return(e) = &block.term else {
            continue;
        };
        returns += 1;
        out.extend(literal_overwrites(e.peel_ts(), prev)?);
    }
    (returns > 0).then_some(out)
}

/// `{ ...prev, a: x }` overwrites `{a}` and preserves every other member;
/// `prev` itself overwrites nothing. Any other shape — a different spread
/// source, a second spread, a key the lowering could not name — proves
/// nothing, because a member it cannot see may be one the deps read.
fn literal_overwrites(e: &Expr, prev: &Var) -> Option<HashSet<Symbol>> {
    if matches!(e, Expr::Var(v) if v == prev) {
        return Some(HashSet::new());
    }
    let Expr::ObjectLit { fields, .. } = e else {
        return None;
    };
    let [(spread, src), rest @ ..] = fields.as_slice() else {
        return None;
    };
    if !spread.starts_with(SPREAD_KEY_PREFIX) || !matches!(src.peel_ts(), Expr::Var(v) if v == prev)
    {
        return None;
    }
    rest.iter()
        .map(|(k, _)| named_key(k).cloned())
        .collect::<Option<HashSet<Symbol>>>()
}

/// A key a `FieldAccess` could actually ask for — every synthetic one (a
/// further spread, a computed key, an accessor) answers `None`.
fn named_key(key: &Symbol) -> Option<&Symbol> {
    (!key.starts_with(SPREAD_KEY_PREFIX) && !key.starts_with('[')).then_some(key)
}

/// The first member a dep reads off state slot `label`: `data.name.first`
/// answers `name`. `None` when the dep is not a plain member chain on that
/// slot — the bare slot included, since every fresh write changes it.
/// `state_vals` must be the must-alias map: a dep rooted at a name the slot's
/// read is bound to on one arm only reads something else on the other (#222).
fn slot_member(
    dep: &Expr,
    label: HookLabel,
    state_vals: &HashMap<Var, HookLabel>,
) -> Option<Symbol> {
    let mut cur = dep.peel_ts();
    let mut first = None;
    while let Expr::FieldAccess { obj, field } = cur {
        first = Some(named_key(field)?.clone());
        cur = obj.peel_ts();
    }
    let rooted = match cur {
        Expr::StateVal(l) => *l == label,
        Expr::Var(v) => state_vals.get(v) == Some(&label),
        _ => false,
    };
    rooted.then_some(first).flatten()
}
