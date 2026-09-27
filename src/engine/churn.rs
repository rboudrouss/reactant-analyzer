//! The program churn graph (ADR-018, ADR-042 §4): a fold over two relations.
//!
//! ```text
//! edge x → y  ≡  "a change of x re-runs an effect that stores a fresh
//!                 reference into y"          a cycle = self-sustaining loop
//! ```
//!
//! Nothing is walked here. The writes are the effect-region rows of
//! `slot_writers`, the re-run triggers the rows of `effect_triggers`, and the
//! graph is the product of the two, per effect, program-wide. Edge strength:
//! - `Must`: the dep on x is the exact slot (must-rerun) ∧ the fresh write to
//!   y sits in a block on all paths of the body (must-reach) ∧ the written
//!   value is `PerRender` (must-change). An all-must cycle ⇒ Error.
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
//! other effect write of the slot program-wide, each taken with the
//! invariant facts that site ran under ([`converges_under_all_writes`],
//! #154). A site in another component has guards this component's env
//! cannot read, so such a slot is never killed; a handler row needs a user
//! event and is not a site. One kill, one proof, for the dep-driven
//! same-slot edge too. The proof reads facts over props only in a component
//! no effect of which reacts to a parent slot: a loop can enter a component
//! through its props only by such a dep, and then the props may move on any
//! cycle through it, whichever edge the cycle takes.
//!
//! **Two arms, one relation** (ADR-020 item 2). A dep-driven edge whose write
//! lands in the very slot its deps read is `self_slot`: the self-churn arm's
//! partition. The cycle search skips it; the self-churn arm reads only it.
//! Every other edge is the graph arm's.

use std::collections::{HashMap, HashSet};

use crate::{
    engine::{
        AnalysisResult, ConvergedEval, EffectTrigger, Freshness, ProgramAnalysisResult, SlotWriter,
        WriterPhase, WriterRegion,
        dominance::on_all_paths,
        guards::{Invariance, WriteSite, converges_under_all_writes, let_bindings, mutated_roots},
        render_deps::written_names,
        setters::{memo_val_labels, resolve_setter_aliases, state_val_labels},
        triggers_of,
        written::reference_part,
    },
    ir::{
        ComponentId, QualifiedSlot, SourceRange,
        cfg::CFG,
        expr::{Expr, SPREAD_KEY_PREFIX},
        hooks::{Arity, HookEntry},
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
    /// The carrying effect has no dependency array.
    pub no_deps: bool,
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

/// Build all churn edges of the program.
pub fn build_edges(result: &ProgramAnalysisResult) -> Vec<ChurnEdge> {
    // Per-component facts the proofs read.
    struct CompCtx<'a> {
        state_vals: HashMap<Var, HookLabel>,
        memo_vals: HashMap<Var, HookLabel>,
        render_lets: HashMap<&'a str, Option<&'a Expr>>,
        mutated: HashSet<Var>,
        /// No effect of the component reacts to another component's slot, so
        /// no loop enters it through its props: they hold across every loop
        /// its edges can be on. One dep versioned by a parent slot, and the
        /// props may move on any cycle through the component — the proofs
        /// then read no fact over them.
        props_hold: bool,
    }
    // Per-effect facts, gathered first so the write sites of every slot are
    // known program-wide before any convergence kill is attempted (see
    // module doc).
    struct EffectFacts<'a> {
        comp: ComponentId,
        comp_result: &'a AnalysisResult<crate::domains::StateValue>,
        body_cfg: &'a CFG,
        effect_label: HookLabel,
        no_deps: bool,
        deps: &'a [Expr],
        triggers: Vec<&'a EffectTrigger>,
        exact_local: HashSet<HookLabel>,
        versioned: HashSet<QualifiedSlot>,
        writes: Vec<&'a SlotWriter>,
    }
    /// One effect write site of a slot, with the body it sits in.
    struct SiteRef<'a> {
        comp: ComponentId,
        cfg: &'a CFG,
        row: &'a SlotWriter,
    }

    let mut sites: HashMap<QualifiedSlot, Vec<SiteRef>> = HashMap::new();
    let mut ctxs: HashMap<ComponentId, CompCtx> = HashMap::new();
    let mut facts: Vec<EffectFacts> = Vec::new();

    // A module name any component writes moves between two runs of any
    // component's effect: the union is program-wide, as `render_deps` reads
    // it.
    let module_written: HashSet<Var> = result.components.values().flat_map(written_names).collect();

    for (&comp, comp_result) in &result.components {
        let cfg = &comp_result.render_cfg;
        let mut mutated = module_written.clone();
        mutated.extend(mutated_roots(cfg));
        for body in comp_result.hooks.iter().filter_map(HookEntry::body_cfg) {
            mutated.extend(mutated_roots(body));
        }
        ctxs.insert(
            comp,
            CompCtx {
                state_vals: resolve_setter_aliases(cfg, &state_val_labels(cfg)),
                memo_vals: resolve_setter_aliases(cfg, &memo_val_labels(cfg)),
                render_lets: let_bindings(cfg),
                mutated,
                props_hold: comp_result.effect_triggers.iter().all(|t| t.slot.0 == comp),
            },
        );
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
            // Mount-only effects fire once: no loop — but only an array the
            // engine knows is empty says so.
            if matches!(deps.list(), Some(d) if d.arity == Arity::Exact(0)) {
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
            for w in &writes {
                sites.entry(node_of(comp, w)).or_default().push(SiteRef {
                    comp,
                    cfg: body_cfg,
                    row: w,
                });
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
            facts.push(EffectFacts {
                comp,
                comp_result,
                body_cfg,
                effect_label: *label,
                // An unreadable deps argument gates the effect by a list the
                // engine cannot use, so it is read the same way as no list at
                // all: the self-edge stays, which is the fire-more direction.
                no_deps: deps.list().is_none(),
                deps: deps.list().map_or(&[][..], |d| d.as_slice()),
                triggers,
                exact_local,
                versioned,
                writes,
            });
        }
    }

    // Deduplicate on (from, to, component, effect): keep the strongest, and
    // among equals the earliest write site, so the row a reader anchors on
    // does not depend on relation order.
    let mut best: HashMap<(QualifiedSlot, QualifiedSlot, ComponentId, HookLabel), ChurnEdge> =
        HashMap::new();
    for f in &facts {
        let ctx = &ctxs[&f.comp];
        let invariance = Invariance {
            render: &ctx.render_lets,
            state_vals: &ctx.state_vals,
            memo_vals: &ctx.memo_vals,
            mutated: &ctx.mutated,
        };
        let exit = f.comp_result.exit_env();
        let mut evaluator = f.comp_result.evaluator();
        let mut eval = |e: &Expr| evaluator.at(&exit, e);
        for w in &f.writes {
            if w.written.fresh == Freshness::Not {
                continue;
            }
            let node = node_of(f.comp, w);
            // Convergence proof: once a written value sits in the slot, do the
            // dominating guards kill this write — under its own write and
            // under every other site's? Edges claim reference churn, so the
            // own write is read as the reference part of its value
            // (references are truthy and non-nullish); another site's write
            // revives with whatever it stores, `null` included.
            let own_value = reference_part(&w.written.value);
            let own = WriteSite {
                cfg: f.body_cfg,
                block: w.block,
                value: &own_value,
                expr: w.written.expr.as_ref(),
            };
            let slot_sites = &sites[&node];
            let foreign_site = slot_sites.iter().any(|s| s.comp != f.comp);
            let others: Vec<WriteSite> = slot_sites
                .iter()
                .filter(|s| !std::ptr::eq(s.row, *w))
                .map(|s| WriteSite {
                    cfg: s.cfg,
                    block: s.row.block,
                    value: &s.row.written.value,
                    expr: s.row.written.expr.as_ref(),
                })
                .collect();
            let killed = node.0 == f.comp
                && !foreign_site
                && converges_under_all_writes(
                    &own,
                    &others,
                    &ctx.state_vals,
                    node.1,
                    &invariance,
                    ctx.props_hold,
                    &exit,
                    &mut eval,
                );
            let fresh_blocks: HashSet<BlockId> = f
                .writes
                .iter()
                .filter(|o| node_of(f.comp, o) == node && o.written.fresh == Freshness::Fresh)
                .filter_map(|o| o.block)
                .collect();
            let must_write = w.written.fresh == Freshness::Fresh
                && w.block.is_some()
                && on_all_paths(f.body_cfg, &fresh_blocks);
            let strength = if must_write {
                EdgeStrength::Must
            } else {
                EdgeStrength::May
            };

            let mut push = |from: QualifiedSlot, strength: EdgeStrength, self_slot: bool| {
                let key = (from, node, f.comp, f.effect_label);
                let edge = ChurnEdge {
                    from,
                    to: node,
                    strength,
                    component: f.comp,
                    effect_label: f.effect_label,
                    write_span: w.span,
                    no_deps: f.no_deps,
                    self_slot,
                };
                best.entry(key)
                    .and_modify(|e| {
                        if (strength, std::cmp::Reverse(pos(w.span)))
                            > (e.strength, std::cmp::Reverse(pos(e.write_span)))
                        {
                            *e = edge.clone();
                        }
                    })
                    .or_insert(edge);
            };

            if f.no_deps {
                // Re-runs after every render → its own write re-triggers it,
                // whichever turn the write runs on (a handler row was dropped
                // above).
                if !killed {
                    push(node, strength, false);
                }
                continue;
            }
            for &l in &f.exact_local {
                let x: QualifiedSlot = (f.comp, l);
                let self_slot = x == node;
                let dropped = killed || (self_slot && !write_can_retrigger(f, &ctx.state_vals, w));
                if !dropped {
                    push(x, strength, self_slot);
                }
            }
            for &x in &f.versioned {
                if f.exact_local.contains(&x.1) && x.0 == f.comp {
                    continue; // already pushed as exact
                }
                let self_slot = x == node && x.0 == f.comp;
                let dropped = killed || (self_slot && !write_can_retrigger(f, &ctx.state_vals, w));
                if !dropped {
                    push(x, EdgeStrength::May, self_slot);
                }
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
