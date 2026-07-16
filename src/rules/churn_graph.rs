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
        hooks::HookEntry,
        types::{HookLabel, Symbol},
    },
};

use super::infinite_loop::{
    Freshness, classify_effect_deps, collect_churn_calls, converges_once_written, on_all_paths,
};
use super::{
    collect_component_setter_vars, collect_fn_bindings, memo_val_labels, resolve_setter_aliases,
    setter_var_labels, state_val_labels,
};

/// A state slot qualified by its owning component.
pub(super) type SlotNode = (Symbol, HookLabel);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum EdgeStrength {
    /// Dep merely versioned by `from`, or the write is conditional/imprecise.
    May,
    /// Exact-slot dep ∧ must-fresh write on all paths.
    Must,
}

#[derive(Debug, Clone)]
pub(super) struct ChurnEdge {
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
pub(super) struct ChurnCycle {
    pub edge_idx: Vec<usize>,
    pub all_must: bool,
    /// The cycle involves more than one component (slot owners or effect
    /// carriers) — severity is capped at Warning: cross-component must-rerun
    /// cannot be proven (prop deps are `Versioned`, never exact).
    pub cross_component: bool,
}

/// Build all churn edges of the program.
pub(super) fn build_churn_graph(result: &ProgramAnalysisResult) -> Vec<ChurnEdge> {
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
        calls: Vec<super::infinite_loop::ChurnSetterCall>,
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
        for (v, (parent, l)) in
            collect_component_setter_vars(cfg, &comp_result.block_states, &comp_result.heap)
        {
            if parent != *comp {
                setter_nodes.insert(v, (parent, l));
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
            // Mount-only effects fire once: no loop.
            if matches!(deps, Some(d) if d.is_empty()) {
                continue;
            }
            let (exact_local, versioned) = match deps {
                None => (HashSet::new(), HashSet::new()),
                Some(d) => classify_effect_deps(d, comp_result, &state_vals, &memo_vals),
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
                no_deps: deps.is_none(),
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
            // slot (see module doc).
            if call.node.0 == *f.comp
                && writer_sites.get(&call.node) == Some(&1)
                && let Some(b) = call.block_id
                && converges_once_written(
                    f.body_cfg,
                    b,
                    &f.state_vals,
                    call.node.1,
                    &call.written,
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
                // self-sustaining. Auto-run callbacks (`.then`) are a known
                // FN here, matching their pre-F5b silence.
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
pub(super) fn find_churn_cycles(edges: &[ChurnEdge]) -> Vec<ChurnCycle> {
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
