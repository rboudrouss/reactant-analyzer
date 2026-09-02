//! F5b — multi-effect churn cycles (ADR-017 §Limitations follow-up).
//!
//! The self-churn arm of `infinite-loop` proves loops confined to one effect
//! and one state slot. A loop spread over several effects (effect A deps
//! `[a]` freshly sets `b`; effect B deps `[b]` freshly sets `a`) leaves every
//! existing arm blind: no slot diverges in the fixpoint (references converge
//! under join) and no single effect both depends on and writes the same slot.
//!
//! This module builds a graph over *qualified* state slots
//! (`(component, label)` — parent slots written through `ComponentSetter`
//! props participate) and finds reference-churn cycles:
//!
//! ```text
//! edge x → y  ≡  "a change of x re-runs an effect that stores a fresh
//!                 reference into y"
//! ```
//!
//! A cycle is a self-sustaining render loop. Edge strength:
//! - `Must`: the dep on x is the exact slot (must-rerun) ∧ the fresh write to
//!   y is on all paths (must-reach) ∧ the written value is `PerRender`
//!   (must-change). An all-must cycle ⇒ Error (triple-must, per the
//!   diagnostic doctrine).
//! - `May`: the dep is merely versioned by x, or the write is conditional /
//!   imprecise.
//!
//! An effect with **no dependency array** re-runs after every render — in
//! particular after the render its own write causes — so a fresh write there
//! is a self-edge `y → y` (a length-1 cycle needing no partner).
//!
//! **Convergence kill**: an edge is dropped when the write provably happens
//! at most once — dominating guards narrow to ⊥ once the written value sits
//! in the slot (`converges_once_written`) — but ONLY when the slot has a
//! single effect write-site program-wide. Another *effect* rewriting the slot
//! can revive the guard on the next automatic round, so the proof would be
//! unsound (FN); handlers cannot (they need a user event, so the loop is not
//! self-sustaining through them) and are not counted.

use std::collections::{HashMap, HashSet};

use crate::{
    engine::ProgramAnalysisResult,
    ir::{
        SourceRange,
        hooks::{Arity, HookEntry},
        types::{HookLabel, Symbol},
    },
};

use super::churn::{
    ChurnSetterCall, Freshness, SlotNode, classify_effect_deps, collect_churn_calls,
    converges_once_written, on_all_paths, reference_part,
};
use super::setters::{
    collect_component_setter_vars, collect_fn_bindings, memo_val_labels, resolve_setter_aliases,
    setter_var_labels, state_val_labels,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::rules) enum EdgeStrength {
    /// Dep merely versioned by `from`, or the write is conditional/imprecise.
    May,
    /// Exact-slot dep ∧ must-fresh write on all paths.
    Must,
}

#[derive(Debug, Clone)]
pub(in crate::rules) struct ChurnEdge {
    pub from: SlotNode,
    pub to: SlotNode,
    pub strength: EdgeStrength,
    /// Component whose effect carries this edge.
    pub component: Symbol,
    pub effect_label: HookLabel,
    pub write_span: Option<SourceRange>,
    /// The carrying effect has no dependency array.
    pub no_deps: bool,
}

/// One churn cycle: indices into the edge list, in cycle order
/// (`edges[i].to == edges[i+1].from`, last wraps to first).
pub(in crate::rules) struct ChurnCycle {
    pub edge_idx: Vec<usize>,
    pub all_must: bool,
    /// The cycle involves more than one component (slot owners or effect
    /// carriers) — severity is capped at Warning: cross-component must-rerun
    /// cannot be proven (prop deps are `Versioned`, never exact).
    pub cross_component: bool,
}

/// Build all churn edges of the program.
pub(in crate::rules) fn build_churn_graph(result: &ProgramAnalysisResult) -> Vec<ChurnEdge> {
    // Per-effect facts, gathered first so write-site counts are global before
    // any convergence kill is attempted (see module doc).
    struct EffectFacts<'a> {
        comp: &'a Symbol,
        comp_result: &'a crate::engine::AnalysisResult<crate::domains::StateValue>,
        body_cfg: &'a crate::ir::cfg::CFG,
        state_vals: HashMap<crate::ir::types::Var, HookLabel>,
        effect_label: HookLabel,
        no_deps: bool,
        exact_local: HashSet<HookLabel>,
        versioned: HashSet<SlotNode>,
        calls: Vec<ChurnSetterCall>,
    }

    let mut writer_sites: HashMap<SlotNode, usize> = HashMap::new();
    let mut facts: Vec<EffectFacts> = Vec::new();

    for (comp, comp_result) in &result.components {
        let cfg = &comp_result.render_cfg;
        let state_vals = resolve_setter_aliases(cfg, &state_val_labels(cfg));
        let memo_vals = resolve_setter_aliases(cfg, &memo_val_labels(cfg));
        let local_setters = resolve_setter_aliases(cfg, &setter_var_labels(cfg));

        let mut setter_nodes: HashMap<crate::ir::types::Var, SlotNode> = local_setters
            .iter()
            .map(|(v, l)| (v.clone(), (comp.clone(), *l)))
            .collect();
        for (v, prop) in
            collect_component_setter_vars(cfg, &comp_result.block_states, &comp_result.heap)
        {
            if prop.component != *comp {
                setter_nodes.insert(v, (prop.component, prop.label));
            }
        }
        if setter_nodes.is_empty() {
            continue;
        }
        let fn_bindings = collect_fn_bindings(cfg);

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
            let (exact_local, versioned) = match deps.list() {
                None => (HashSet::new(), HashSet::new()),
                Some(d) => classify_effect_deps(d.as_slice(), comp_result, &state_vals, &memo_vals),
            };
            let mut calls = Vec::new();
            collect_churn_calls(
                body_cfg,
                &setter_nodes,
                &fn_bindings,
                comp_result,
                1,
                true,
                &mut calls,
            );
            // Every write site counts — a `setX(null)` (freshness Not) still
            // revives guards, so it must block convergence kills on X.
            for c in &calls {
                *writer_sites.entry(c.node.clone()).or_default() += 1;
            }
            facts.push(EffectFacts {
                comp,
                comp_result,
                body_cfg,
                state_vals: state_vals.clone(),
                effect_label: *label,
                // An unreadable deps argument gates the effect by a list the
                // engine cannot use, so it is read the same way as no list at
                // all: the self-edge stays, which is the fire-more direction.
                no_deps: deps.list().is_none(),
                exact_local,
                versioned,
                calls,
            });
        }
    }

    // Deduplicate on (from, to, component, effect): keep the strongest.
    let mut best: HashMap<(SlotNode, SlotNode, Symbol, HookLabel), ChurnEdge> = HashMap::new();
    for f in &facts {
        for call in &f.calls {
            if call.freshness == Freshness::Not {
                continue;
            }
            // Convergence kill — sound only for a single-effect-writer local
            // slot (see module doc). Edges claim reference churn, so the
            // guard proof runs against the reference part of the written
            // value only (references are truthy and non-nullish).
            if call.node.0 == *f.comp
                && writer_sites.get(&call.node) == Some(&1)
                && let Some(b) = call.block_id
                && converges_once_written(
                    f.body_cfg,
                    b,
                    &f.state_vals,
                    call.node.1,
                    &reference_part(&call.written),
                    call.written_expr.as_ref(),
                    f.comp_result,
                )
            {
                continue;
            }
            let fresh_blocks: HashSet<crate::ir::types::BlockId> = f
                .calls
                .iter()
                .filter(|c| c.node == call.node && c.freshness == Freshness::Fresh)
                .filter_map(|c| c.block_id)
                .collect();
            let must_write = call.freshness == Freshness::Fresh
                && !fresh_blocks.is_empty()
                && on_all_paths(f.body_cfg, &fresh_blocks);

            let mut push = |from: SlotNode, strength: EdgeStrength| {
                let key = (
                    from.clone(),
                    call.node.clone(),
                    f.comp.clone(),
                    f.effect_label,
                );
                let edge = ChurnEdge {
                    from,
                    to: call.node.clone(),
                    strength,
                    component: f.comp.clone(),
                    effect_label: f.effect_label,
                    write_span: call.span,
                    no_deps: f.no_deps,
                };
                best.entry(key)
                    .and_modify(|e| {
                        if strength > e.strength {
                            *e = edge.clone();
                        }
                    })
                    .or_insert(edge);
            };

            if f.no_deps {
                // Re-runs after every render → its own write re-triggers it.
                // Top-level calls only (`block_id` set): a write nested in a
                // callback may be event-driven (`addEventListener`) — not
                // self-sustaining. Auto-run callbacks (`.then`) are a known FN here.
                if call.block_id.is_some() {
                    push(
                        call.node.clone(),
                        if must_write {
                            EdgeStrength::Must
                        } else {
                            EdgeStrength::May
                        },
                    );
                }
                continue;
            }
            for l in &f.exact_local {
                let x: SlotNode = (f.comp.clone(), *l);
                // Dep-driven same-slot churn is the self-churn arm's domain.
                if x == call.node {
                    continue;
                }
                push(
                    x,
                    if must_write {
                        EdgeStrength::Must
                    } else {
                        EdgeStrength::May
                    },
                );
            }
            for x in &f.versioned {
                if f.exact_local.contains(&x.1) && x.0 == *f.comp {
                    continue; // already pushed as exact
                }
                // Local same-slot versioned churn: self-churn arm's Warning.
                if *x == call.node && x.0 == *f.comp {
                    continue;
                }
                push(x.clone(), EdgeStrength::May);
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
    edges
}

/// Find churn cycles: first in the must-only subgraph (all-must cycles),
/// then in the full graph, skipping regions already reported.
pub(in crate::rules) fn find_churn_cycles(edges: &[ChurnEdge]) -> Vec<ChurnCycle> {
    let must_idx: Vec<usize> = (0..edges.len())
        .filter(|&i| edges[i].strength == EdgeStrength::Must)
        .collect();
    let all_idx: Vec<usize> = (0..edges.len()).collect();

    let mut cycles = Vec::new();
    let mut covered_nodes: HashSet<SlotNode> = HashSet::new();

    for cyc in cycles_in(edges, &must_idx) {
        for &i in &cyc {
            covered_nodes.insert(edges[i].from.clone());
            covered_nodes.insert(edges[i].to.clone());
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
    let mut comps: HashSet<&Symbol> = HashSet::new();
    for &i in &edge_idx {
        comps.insert(&edges[i].from.0);
        comps.insert(&edges[i].to.0);
        comps.insert(&edges[i].component);
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
    let mut nodes: Vec<&SlotNode> = subset
        .iter()
        .flat_map(|&i| [&edges[i].from, &edges[i].to])
        .collect();
    nodes.sort();
    nodes.dedup();
    let node_id: HashMap<&SlotNode, usize> =
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
    node_id: &HashMap<&SlotNode, usize>,
) -> Vec<Vec<usize>> {
    struct St<'a> {
        adj: &'a [Vec<usize>],
        edges: &'a [ChurnEdge],
        node_id: &'a HashMap<&'a SlotNode, usize>,
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
    node_id: &HashMap<&SlotNode, usize>,
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

/// The program's churn graph: every edge plus the cycles found in it.
///
/// Whole-program data — `build` walks every component — so it is computed once
/// per program and shared by every component's rule pass (see
/// [`crate::rules::api::cache::ProgramCache`]).
pub(in crate::rules) struct ChurnGraph {
    pub edges: Vec<ChurnEdge>,
    pub cycles: Vec<ChurnCycle>,
}

#[cfg(test)]
thread_local! {
    /// Test-only count of [`ChurnGraph::build`] calls on the current thread —
    /// the guard for issue #86 (built once per program, never once per
    /// component). Thread-local, so parallel tests never observe each other.
    pub(in crate::rules) static BUILDS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

impl ChurnGraph {
    pub(in crate::rules) fn build(result: &ProgramAnalysisResult) -> Self {
        #[cfg(test)]
        BUILDS.with(|n| n.set(n.get() + 1));
        let edges = build_churn_graph(result);
        let cycles = if edges.is_empty() {
            Vec::new()
        } else {
            find_churn_cycles(&edges)
        };
        ChurnGraph { edges, cycles }
    }
}

// ── Naming and the per-component projection ───────────────────────────────────
//
// Both readers of the graph — the native F5b arm and the Tier-A `churn_cycles`
// anchor — need the same two things: a qualified display name for a slot node,
// and the cycle rendered as a path. They are here so the two cannot drift
// (ADR-027 §1: the fact is computed once).

/// Slot display names per component, resolved lazily. Building the alias table
/// for a component is not free, and a cycle names the same slots repeatedly.
pub(in crate::rules) type NodeNames = HashMap<Symbol, HashMap<crate::ir::types::Var, HookLabel>>;

/// Display name of a qualified slot: `` `count` `` locally,
/// `` `count` of `Parent` `` for another component's slot.
pub(in crate::rules) fn node_display(
    node: &SlotNode,
    component: &Symbol,
    result: &ProgramAnalysisResult,
    names: &mut NodeNames,
) -> String {
    let map = names.entry(node.0.clone()).or_insert_with(|| {
        result
            .components
            .get(&node.0)
            .map(|r| {
                super::setters::resolve_setter_aliases(
                    &r.render_cfg,
                    &super::setters::state_val_labels(&r.render_cfg),
                )
            })
            .unwrap_or_default()
    });
    let base = crate::rules::state_slot_name(node.1, map);
    if node.0 == *component {
        base
    } else {
        format!("{base} of `{}`", node.0)
    }
}

/// The cycle as `a → b → a`: every edge's source, then back to the first.
pub(in crate::rules) fn cycle_path(
    edges: &[ChurnEdge],
    cycle: &ChurnCycle,
    component: &Symbol,
    result: &ProgramAnalysisResult,
    names: &mut NodeNames,
) -> String {
    let mut parts: Vec<String> = cycle
        .edge_idx
        .iter()
        .map(|&i| node_display(&edges[i].from, component, result, names))
        .collect();
    if let Some(&first) = cycle.edge_idx.first() {
        parts.push(node_display(&edges[first].from, component, result, names));
    }
    parts.join(" → ")
}

/// One `churn_cycles` row: a cycle of the program graph, seen from the effect
/// of THIS component that carries one of its edges.
///
/// Per-component attribution mirrors the native arm — an effect in another
/// component reports in that component's own pass — so the relation stays
/// single-anchored on the component under analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::rules) struct CycleRow {
    /// The cycle rendered as `a → b → a`.
    pub path: String,
    /// The cycle spans more than one component (slot owners or effect
    /// carriers). Exact: a fold of the node table the graph already built.
    pub cross_component: bool,
    /// Every edge of the cycle is a Must edge. Exact, and the graph's own
    /// must-claim — it is what the native arm certifies an Error from.
    pub all_must: bool,
    /// The effect of this component that carries the edge.
    pub effect: HookLabel,
    /// The carrying edge's write site — the row's identity (ADR-024). A row
    /// exists only when there is one.
    pub span: SourceRange,
}

/// The `churn_cycles` rows of `component`, deterministically ordered.
///
/// **Spanless carrying edges yield no row.** ADR-024 makes the write site the
/// row's identity, and a row with nowhere to point is a finding a reader cannot
/// act on; the graph's own `write_span` is an `Option`, so this is a recorded
/// missed-findings channel, never a wrong one.
pub(in crate::rules) fn collect_cycle_rows(
    graph: &ChurnGraph,
    result: &ProgramAnalysisResult,
    component: &Symbol,
) -> Vec<CycleRow> {
    let mut names: NodeNames = HashMap::new();
    let mut rows: Vec<CycleRow> = Vec::new();
    for cycle in &graph.cycles {
        let path = cycle_path(&graph.edges, cycle, component, result, &mut names);
        for &i in &cycle.edge_idx {
            let e = &graph.edges[i];
            if e.component != *component {
                continue;
            }
            let Some(span) = e.write_span else {
                continue;
            };
            rows.push(CycleRow {
                path: path.clone(),
                cross_component: cycle.cross_component,
                all_must: cycle.all_must,
                effect: e.effect_label,
                span,
            });
        }
    }
    // A cycle visits each slot once, so one effect carries at most one of its
    // edges — but two cycles through the same write site would otherwise emit
    // the same finding twice.
    rows.sort_by(|a, b| {
        (a.span.pos_key(), a.effect, &a.path).cmp(&(b.span.pos_key(), b.effect, &b.path))
    });
    rows.dedup();
    rows
}

#[cfg(test)]
mod row_tests {
    use super::*;

    fn span(line: u32) -> SourceRange {
        SourceRange {
            file: crate::ir::FileTable::default().intern(std::path::Path::new("t.tsx")),
            line,
            col: 1,
        }
    }

    fn edge(
        from: (&str, HookLabel),
        to: (&str, HookLabel),
        carrier: &str,
        effect: HookLabel,
        write_span: Option<SourceRange>,
    ) -> ChurnEdge {
        ChurnEdge {
            from: (from.0.to_string(), from.1),
            to: (to.0.to_string(), to.1),
            strength: EdgeStrength::May,
            component: carrier.to_string(),
            effect_label: effect,
            write_span,
            no_deps: false,
        }
    }

    fn graph(edges: Vec<ChurnEdge>, cross_component: bool, all_must: bool) -> ChurnGraph {
        let edge_idx = (0..edges.len()).collect();
        ChurnGraph {
            edges,
            cycles: vec![ChurnCycle {
                edge_idx,
                all_must,
                cross_component,
            }],
        }
    }

    fn empty_program() -> ProgramAnalysisResult {
        ProgramAnalysisResult {
            components: Default::default(),
            shared_state: crate::domains::stores::SharedStateStore::new(),
            call_graph: crate::engine::ComponentCallGraph::new(),
            recursive_components: Default::default(),
            stats: Default::default(),
            file_table: Default::default(),
            module_table: Default::default(),
            function_registry: Default::default(),
            phase1_reached: Default::default(),
        }
    }

    /// Per-component attribution: an edge carried by another component's
    /// effect reports in that component's own pass, never here.
    #[test]
    fn only_edges_this_component_carries_produce_rows() {
        let g = graph(
            vec![
                edge(("Parent", 0), ("Child", 1), "Child", 7, Some(span(3))),
                edge(("Child", 1), ("Parent", 0), "Parent", 9, Some(span(4))),
            ],
            true,
            false,
        );
        let prog = empty_program();
        let rows = collect_cycle_rows(&g, &prog, &"Child".to_string());
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].effect, 7);
        assert!(rows[0].cross_component);
    }

    /// ADR-024 makes the write site the row's identity, so an edge the graph
    /// could not place produces no row at all — a missed finding, never one
    /// pointing nowhere.
    #[test]
    fn a_spanless_carrying_edge_yields_no_row() {
        let g = graph(
            vec![
                edge(("C", 0), ("C", 1), "C", 7, None),
                edge(("C", 1), ("C", 0), "C", 8, Some(span(4))),
            ],
            false,
            true,
        );
        let prog = empty_program();
        let rows = collect_cycle_rows(&g, &prog, &"C".to_string());
        assert_eq!(rows.len(), 1, "the spanless edge must drop: {rows:?}");
        assert_eq!(rows[0].effect, 8);
        assert!(rows[0].all_must);
    }

    /// Two cycles through the same write site are one finding, not two.
    #[test]
    fn the_same_write_site_in_two_cycles_is_one_row() {
        let e = edge(("C", 0), ("C", 1), "C", 7, Some(span(3)));
        let back = edge(("C", 1), ("C", 0), "C", 8, Some(span(4)));
        let g = ChurnGraph {
            edges: vec![e, back],
            cycles: vec![
                ChurnCycle {
                    edge_idx: vec![0, 1],
                    all_must: false,
                    cross_component: false,
                },
                ChurnCycle {
                    edge_idx: vec![0, 1],
                    all_must: false,
                    cross_component: false,
                },
            ],
        };
        let prog = empty_program();
        let rows = collect_cycle_rows(&g, &prog, &"C".to_string());
        assert_eq!(rows.len(), 2, "one row per carrying effect: {rows:?}");
    }
}
