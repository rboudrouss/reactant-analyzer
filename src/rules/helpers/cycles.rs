//! Naming and the per-component projection of the program churn graph.
//!
//! The graph itself is the engine's ([`crate::engine::churn`], ADR-042 §4).
//! Both readers of it — the native F5b arm of `infinite-loop` and the Tier-A
//! `churn_cycles` anchor — need the same two things: a qualified display name
//! for a slot node, and the cycle rendered as a path. They are here so the two
//! cannot drift (ADR-027 §1: the fact is computed once), and here rather than
//! in the engine because a display name is a rendering, not a fact.

use std::collections::HashMap;

use crate::{
    engine::{
        ProgramAnalysisResult,
        churn::{ChurnCycle, ChurnEdge, ChurnGraph},
    },
    ir::{ComponentId, QualifiedSlot, SourceRange, types::HookLabel},
};

/// Slot display names per component, resolved lazily. Building the alias table
/// for a component is not free, and a cycle names the same slots repeatedly.
pub(in crate::rules) type NodeNames =
    HashMap<ComponentId, HashMap<crate::ir::types::Var, HookLabel>>;

/// Display name of a qualified slot: `` `count` `` locally,
/// `` `count` of `Parent` `` for another component's slot.
pub(in crate::rules) fn node_display(
    node: &QualifiedSlot,
    component: ComponentId,
    result: &ProgramAnalysisResult,
    names: &mut NodeNames,
) -> String {
    let map = names.entry(node.0).or_insert_with(|| {
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
    if node.0 == component {
        base
    } else {
        format!("{base} of `{}`", result.display_name(node.0))
    }
}

/// The cycle as `a → b → a`: every edge's source, then back to the first.
pub(in crate::rules) fn cycle_path(
    edges: &[ChurnEdge],
    cycle: &ChurnCycle,
    component: ComponentId,
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
    component: ComponentId,
) -> Vec<CycleRow> {
    let mut names: NodeNames = HashMap::new();
    let mut rows: Vec<CycleRow> = Vec::new();
    for cycle in &graph.cycles {
        let path = cycle_path(&graph.edges, cycle, component, result, &mut names);
        for &i in &cycle.edge_idx {
            let e = &graph.edges[i];
            if e.component != component {
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
    use crate::engine::churn::EdgeStrength;

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
            from: (crate::test_support::named(from.0), from.1),
            to: (crate::test_support::named(to.0), to.1),
            strength: EdgeStrength::May,
            component: crate::test_support::named(carrier),
            effect_label: effect,
            write_span,
            no_deps: false,
            self_slot: false,
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
        ProgramAnalysisResult::default()
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
        let rows = collect_cycle_rows(&g, &prog, crate::test_support::named("Child"));
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
        let rows = collect_cycle_rows(&g, &prog, crate::test_support::named("C"));
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
        let rows = collect_cycle_rows(&g, &prog, crate::test_support::named("C"));
        assert_eq!(rows.len(), 2, "one row per carrying effect: {rows:?}");
    }
}
