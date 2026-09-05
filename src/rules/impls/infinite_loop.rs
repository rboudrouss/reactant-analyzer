use crate::rules::RuleCtx;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    engine::ProgramAnalysisResult,
    ir::{
        SourceRange,
        cfg::CFG,
        hooks::{Arity, HookEntry},
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use crate::engine::setters::SetterCallPhase;
use crate::rules::api::query::must_effect_cycle;
use crate::rules::helpers::churn::{
    ChurnSetterCall, Freshness, classify_effect_deps, collect_churn_calls, converges_once_written,
    reference_part, write_can_retrigger,
};
use crate::rules::helpers::churn_graph::{
    ChurnEdge, ChurnGraph, NodeNames, cycle_path, node_display,
};
use crate::rules::{
    Certified, Diagnostic, MustResult, OnAllPaths, Rule, Severity, all_deps_provably_stable,
    all_setter_labels, collect_fn_bindings, collect_setter_calls, collect_setter_calls_with_extra,
    memo_val_labels, must_on_all_paths, resolve_setter_aliases, setter_var_labels, state_slot_name,
    state_val_labels,
};

/// Fires when an effect causes an infinite render loop.
///
/// - `"infinite-loop"` local state widens in fixpoint.
/// - `"cross-component-infinite-loop"` parent state widens via ComponentSetter prop.
///
/// Effects with `deps: []` (mount-only) are excluded. Effects with all-unstable deps
/// are treated as no-deps. If parent not in results, cross fires as Warning.
pub struct InfiniteLoop;

impl InfiniteLoop {
    const NAME: &'static str = "infinite-loop";
}

impl Rule for InfiniteLoop {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        use crate::engine::HookKind;
        // A render→effect→setState cycle needs both a state slot and an effect.
        (crate::rules::has_hook_kind(result, component, HookKind::State)
            && crate::rules::has_hook_kind(result, component, HookKind::Effect))
        .then_some(crate::rules::SafeCheck {
            rule: Self::NAME,
            message: "no effect diverges into an infinite render loop",
        })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let comp_result = &result.components[component];

        // Authoritative setter map: base setters plus alias chains
        // (`const s1 = setX`, `s2 = s1`) resolved across the render and every
        // hook body — a setter called in an effect through an alias must still
        // be attributed to its state slot (else a real loop goes unflagged).
        let local_setter_labels: HashMap<Var, HookLabel> = all_setter_labels(comp_result);
        let state_names = state_val_labels(&comp_result.render_cfg);

        // ComponentSetter props, excluding self-references.
        let cs_vars: HashMap<Var, crate::engine::setters::SetterProp> =
            crate::rules::cross_component_setters(comp_result, component);

        let mut all_setter_vars: HashSet<Var> = local_setter_labels.keys().cloned().collect();
        all_setter_vars.extend(cs_vars.keys().cloned());

        if all_setter_vars.is_empty() {
            return vec![];
        }

        let render_fn_bindings: HashMap<Var, Arc<CFG>> =
            collect_fn_bindings(&comp_result.render_cfg);
        let mut diags = Vec::new();
        // Effects already reported by the fixpoint/cross arms — the churn
        // cycle arm skips them (same outage class, one report per effect).
        let mut reported_effects: HashSet<HookLabel> = HashSet::new();

        for hook in &comp_result.hooks {
            let HookEntry::Effect {
                label: eff_label,
                body_cfg,
                deps,
                ..
            } = hook
            else {
                continue;
            };

            // Mount-only: fires once, no loop. Only an array the engine knows
            // is empty says so.
            if matches!(deps.list(), Some(d) if d.arity == Arity::Exact(0)) {
                continue;
            }
            // A deps array gates the effect for good only when EVERY dep is
            // provably stable (React re-runs on ANY changed dep — OR
            // semantics). One stable dep among moving ones gates nothing, and
            // a ⊤/`Versioned` dep is never provably stable (ADR-021 §5: the
            // shipped ⊤ FN, plus its all-vs-any quantifier sibling).
            if let Some(dep_exprs) = deps.list()
                // A ∀ that suppresses must range over the whole list: a
                // flattened spread hides elements that may well move.
                && dep_exprs.arity.exact().is_some()
                && all_deps_provably_stable(dep_exprs.as_slice(), comp_result)
            {
                continue;
            }

            let calls =
                collect_setter_calls_with_extra(body_cfg, &all_setter_vars, 1, &render_fn_bindings);

            for call in &calls {
                // A write only a registered event listener reaches does not
                // close the loop: it needs a user event per iteration, which
                // is the churn graph's own reason for excluding handlers.
                // Before ADR-034 §4 the walk computed this class and the
                // collapse threw it away, so `addEventListener('keydown', h)`
                // read as an effect-body write (#93).
                if call.class == SetterCallPhase::Handler {
                    continue;
                }
                if let Some(&state_label) = local_setter_labels.get(&call.var) {
                    // ── Intra ─────────────────────────────────────────────────
                    if !comp_result.widen_trace.contains_key(&state_label) {
                        continue; // state didn't diverge → bounded
                    }
                    let writes = comp_result.effect_setter_writes.get(state_label);
                    if !writes.is_bottom_value() && !writes.is_unbounded() {
                        continue; // write bounded → narrowing held the growth
                    }

                    let deps_note = if deps.is_declared() {
                        " (its deps do not provably gate it, so the effect can re-run every render)"
                    } else {
                        ""
                    };
                    let mut diag = Diagnostic::warn(
                        "infinite-loop",
                        format!(
                            "this effect keeps pushing state {}{} to new values \
                             on every run. Potential infinite render loop",
                            state_slot_name(state_label, &state_names),
                            deps_note
                        ),
                    )
                    .with_label(state_label);

                    if let Some(r) = comp_result.effect_info.get(eff_label).and_then(|i| i.span) {
                        diag = diag.with_range(r);
                    }

                    let setter_vars_for_label: HashSet<Var> = local_setter_labels
                        .iter()
                        .filter(|&(_, l)| *l == state_label)
                        .map(|(v, _)| v.clone())
                        .collect();
                    for h in &comp_result.hooks {
                        if let HookEntry::Handler {
                            label: h_label,
                            event,
                            body_cfg: h_cfg,
                            ..
                        } = h
                            && !collect_setter_calls(h_cfg, &setter_vars_for_label, 1).is_empty()
                        {
                            let h_span = comp_result.handler_info.get(h_label).and_then(|i| i.span);
                            diag = diag.with_step(
                                crate::rules::Step::Handler {
                                    event: event.clone(),
                                    slot: state_label,
                                },
                                Some(*h_label),
                                h_span,
                                &|l| state_slot_name(l, &state_names),
                            );
                        }
                    }

                    // Fixpoint evidence (ADR-019): which effects were writing
                    // the slot when it widened, and at which iteration.
                    diag = diag.with_notes(crate::rules::api::witness::slot_history(
                        comp_result,
                        state_label,
                        &|l| state_slot_name(l, &state_names),
                    ));

                    reported_effects.insert(*eff_label);
                    diags.push(diag);
                } else if let Some(prop) = cs_vars.get(&call.var) {
                    // ── Cross-component ────────────────────────────────────────
                    let (parent_comp, parent_label) = (&prop.component, prop.label);
                    let shared_write = result.shared_state.get(parent_comp, parent_label);
                    if shared_write.is_bottom_value() {
                        continue; // setter not reached in semantic analysis
                    }
                    if !shared_write.is_unbounded() {
                        continue; // write is bounded → no divergence
                    }

                    let deps_note = if deps.is_declared() {
                        " (its deps do not provably gate it, so the effect can re-run every render)"
                    } else {
                        ""
                    };
                    let msg = format!(
                        "this effect calls `{}`, a state setter of parent `{}`{}. \
                         Parent re-renders → child re-renders → effect fires again: infinite loop",
                        call.var, parent_comp, deps_note
                    );
                    let mut diag = Diagnostic::warn("cross-component-infinite-loop", msg)
                        .with_label(*eff_label);

                    if let Some(r) = comp_result.effect_info.get(eff_label).and_then(|i| i.span) {
                        diag = diag.with_range(r);
                    }
                    reported_effects.insert(*eff_label);
                    diags.push(diag);
                }
            }
        }

        // ── F5b: multi-effect churn cycles (see churn_graph.rs) ───────────────
        // The graph is whole-program data: read it from the ctx cache, which
        // builds it once for the run (issue #86).
        let (cycle_diags, covered) =
            check_multi_effect_cycles(ctx.cache().churn(), result, component, &reported_effects);
        diags.extend(cycle_diags);
        // Self-churn arm last: its Info branch skips writes a cycle covers.
        diags.extend(check_object_churn(result, component, &covered));

        diags
    }
}

/// F5b — report multi-effect churn cycles touching `component`'s effects.
///
/// One diagnostic per effect of `component` carrying a cycle edge (an effect
/// in another component reports in that component's own check). Also returns
/// the `(effect label, local state label)` writes covered by a reported
/// cycle so the self-churn Info arm doesn't duplicate them.
fn check_multi_effect_cycles(
    graph: &ChurnGraph,
    result: &ProgramAnalysisResult,
    component: &Symbol,
    reported_effects: &HashSet<HookLabel>,
) -> (Vec<Diagnostic>, HashSet<(HookLabel, HookLabel)>) {
    let (edges, cycles) = (&graph.edges, &graph.cycles);
    if edges.is_empty() {
        return (Vec::new(), HashSet::new());
    }
    let comp_result = &result.components[component];

    let mut diags = Vec::new();
    let mut covered: HashSet<(HookLabel, HookLabel)> = HashSet::new();
    // Slot display names, resolved lazily per involved component.
    let mut names: NodeNames = HashMap::new();

    for cycle in cycles {
        let cyc: Vec<&ChurnEdge> = cycle.edge_idx.iter().map(|&i| &edges[i]).collect();
        let path = cycle_path(edges, cycle, component, result, &mut names);
        for e in &cyc {
            if e.component != *component {
                continue;
            }
            if e.to.0 == *component {
                covered.insert((e.effect_label, e.to.1));
            }
            if reported_effects.contains(&e.effect_label) {
                continue; // fixpoint/cross arm already flagged this effect
            }

            // Cross-component must-rerun is unprovable (prop deps are
            // `Versioned`, never the exact slot) → Warning ceiling. An all-must
            // intra-component cycle mints the proof — the only path to Error.
            let rule = if cycle.cross_component {
                "cross-component-infinite-loop"
            } else {
                "infinite-loop"
            };
            let cycle_proof = match must_effect_cycle(edges, cycle) {
                MustResult::All(c) => Some(c),
                _ => None,
            };

            let to_name = node_display(&e.to, component, result, &mut names);
            let msg = if cyc.len() == 1 && e.no_deps {
                format!(
                    "this effect has no dependency array and stores a fresh \
                     reference into state {to_name}, so it re-runs after every \
                     render and re-triggers itself: infinite render loop"
                )
            } else if cyc.len() == 1 {
                format!(
                    "this effect stores a fresh reference into state {to_name} \
                     which its own deps react to, so the re-render runs it again: \
                     infinite render loop"
                )
            } else if cycle.all_must {
                format!(
                    "these effects form a state-update cycle ({path}) where each \
                     step stores a fresh reference that re-runs the next \
                     effect: infinite render loop"
                )
            } else {
                format!(
                    "these effects may form a state-update cycle ({path}) where \
                     each step may store a fresh reference that re-runs the \
                     next effect: possible infinite render loop"
                )
            };

            let mut diag = match cycle_proof {
                Some(proof) => Diagnostic::error(rule, proof, msg),
                None => Diagnostic::warn(rule, msg),
            }
            .with_label(e.effect_label);
            if let Some(r) = comp_result
                .effect_info
                .get(&e.effect_label)
                .and_then(|i| i.span)
            {
                diag = diag.with_range(r);
            }
            if let Some(r) = e.write_span {
                diag = diag.with_step(
                    crate::rules::Step::Write {
                        slot: e.to.1,
                        value: crate::rules::ValueClass::Fresh,
                    },
                    Some(e.effect_label),
                    Some(r),
                    // Qualified display (may name a parent component's slot).
                    &|_| to_name.clone(),
                );
            }
            // Point at the cycle's other steps living in this component.
            for other in &cyc {
                if other.effect_label == e.effect_label || other.component != *component {
                    continue;
                }
                let other_from = node_display(&other.from, component, result, &mut names);
                let other_to = node_display(&other.to, component, result, &mut names);
                diag = diag.with_step(
                    crate::rules::Step::CycleEdge {
                        from: other_from,
                        to: other_to,
                    },
                    Some(other.effect_label),
                    other.write_span,
                    &crate::rules::api::witness::fallback_name,
                );
            }
            diags.push(diag);
        }
    }
    (diags, covered)
}

// ── Object-churn arm (ADR-017) ────────────────────────────────────────────────
//
// `useEffect(() => setObj({...obj}), [obj])` never widens: the reference slot
// converges (`join(PerRender, PerRender)`), so the fixpoint arm above is blind
// to it. Certainty comes from *dep structure* instead: a dep that is the state
// slot itself must-changes when its setter stores a must-fresh reference.
//
// Stratification (Error = all-must, per the diagnostic doctrine):
// - Error:   dep is slot X exactly ∧ setX(fresh) on ALL paths of the body
// - Warning: dep versioned by X (alias/memo/field) ∧ body may-call setX(fresh?)
// - Info:    effect deps on object state but freshly sets a DIFFERENT object
//            state — cross-effect cycles are not analyzed (FN-flavor limit)

fn check_object_churn(
    result: &ProgramAnalysisResult,
    component: &Symbol,
    covered: &HashSet<(HookLabel, HookLabel)>,
) -> Vec<Diagnostic> {
    let comp_result = &result.components[component];
    let cfg = &comp_result.render_cfg;
    let state_vals = resolve_setter_aliases(cfg, &state_val_labels(cfg));
    let setter_labels = resolve_setter_aliases(cfg, &setter_var_labels(cfg));
    let memo_vals = resolve_setter_aliases(cfg, &memo_val_labels(cfg));
    if setter_labels.is_empty() {
        return vec![];
    }
    // This arm is single-effect/single-component: local setters only.
    let setter_nodes: HashMap<Var, crate::rules::helpers::churn::SlotNode> = setter_labels
        .iter()
        .map(|(v, l)| (v.clone(), (component.clone(), *l)))
        .collect();
    let render_fn_bindings = collect_fn_bindings(cfg);
    let mut diags = Vec::new();

    for hook in &comp_result.hooks {
        let HookEntry::Effect {
            label: eff_label,
            body_cfg,
            deps,
            ..
        } = hook
        else {
            continue;
        };
        let Some(dep_exprs) = deps.list() else {
            continue;
        };
        if dep_exprs.arity == Arity::Exact(0) {
            continue; // mount-only
        }

        let (exact, versioned_qualified) =
            classify_effect_deps(dep_exprs.as_slice(), comp_result, &state_vals, &memo_vals);
        // Self-churn is intra-component: keep only own slots.
        let versioned: HashSet<HookLabel> = versioned_qualified
            .into_iter()
            .filter(|(c, _)| c == component)
            .map(|(_, l)| l)
            .collect();
        if exact.is_empty() && versioned.is_empty() {
            continue;
        }

        let mut calls: Vec<ChurnSetterCall> = Vec::new();
        collect_churn_calls(
            body_cfg,
            &setter_nodes,
            &render_fn_bindings,
            comp_result,
            1,
            true,
            &mut calls,
        );

        // Strongest verdict per state label.
        // Best object-churn finding per slot: (severity, call span, Error proof).
        type ChurnBest = (Severity, Option<SourceRange>, Option<Certified<OnAllPaths>>);
        let mut best: HashMap<HookLabel, ChurnBest> = HashMap::new();
        for call in &calls {
            let state_label = call.node.1;
            if call.freshness == Freshness::Not {
                continue;
            }
            // Convergence proof (fetch-once pattern): once the written value
            // sits in the slot, do the dominating guards kill this call?
            // `if (user === null) setUser({...})` → narrowing the written
            // (non-null, truthy) value through the guard yields ⊥ → the set
            // fires at most once, no loop.
            //
            // This arm claims REFERENCE churn, and only a stored reference —
            // always truthy, never nullish — can sustain the loop. Project
            // the written value onto its reference slot so an opaque `f()`
            // result (⊤ elsewhere) stays provable: if the guard dies under
            // every reference, the reference-churn loop cannot re-fire.
            if let Some(b) = call.block_id
                && converges_once_written(
                    body_cfg,
                    b,
                    &state_vals,
                    state_label,
                    &reference_part(&call.written),
                    call.written_expr.as_ref(),
                    comp_result,
                )
            {
                continue;
            }
            // The object-churn Error anchors on the fresh write being on all
            // paths — routed through the must-primitive that mints the proof.
            let (sev, churn_proof) =
                if exact.contains(&state_label) || versioned.contains(&state_label) {
                    // A functional update that spreads its own parameter leaves
                    // every member it does not name `Object.is`-equal, so deps
                    // reading only those cannot be re-triggered by it (#90).
                    if !write_can_retrigger(
                        dep_exprs.as_slice(),
                        component,
                        state_label,
                        &state_vals,
                        &memo_vals,
                        call.written_expr.as_ref(),
                        comp_result,
                    ) {
                        continue;
                    }
                    let fresh_blocks: HashSet<BlockId> = calls
                        .iter()
                        .filter(|c| c.node == call.node && c.freshness == Freshness::Fresh)
                        .filter_map(|c| c.block_id)
                        .collect();
                    if exact.contains(&state_label)
                        && call.freshness == Freshness::Fresh
                        && !fresh_blocks.is_empty()
                    {
                        match must_on_all_paths(body_cfg, &fresh_blocks) {
                            MustResult::All(c) => (Severity::Error, Some(c)),
                            _ => (Severity::Warning, None),
                        }
                    } else {
                        (Severity::Warning, None)
                    }
                } else {
                    // Sets a different object state freshly while depending on
                    // object state: a multi-effect cycle candidate. The churn
                    // graph (F5b) analyzes those; when it reported a cycle for
                    // this write the Info would be a duplicate — skip. Otherwise
                    // keep it: deps may be too imprecise to close a real cycle.
                    if covered.contains(&(*eff_label, state_label)) {
                        continue;
                    }
                    (Severity::Info, None)
                };
            // Severity has no Ord: rank Error > Warning > Info manually.
            let rank = |s: Severity| match s {
                Severity::Error => 2,
                Severity::Warning => 1,
                Severity::Info => 0,
            };
            // Rank ties break on earliest source position: call collection
            // follows HashMap block order, so "first collected" is not
            // deterministic across runs.
            let pos = |s: Option<SourceRange>| s.map_or((u32::MAX, u32::MAX), |r| r.pos_key());
            let replace = match best.get(&state_label) {
                None => true,
                Some(entry) => {
                    rank(sev) > rank(entry.0)
                        || (rank(sev) == rank(entry.0) && pos(call.span) < pos(entry.1))
                }
            };
            if replace {
                best.insert(state_label, (sev, call.span, churn_proof));
            }
        }

        for (state_label, (sev, call_span, proof)) in best {
            let eff_span = comp_result.effect_info.get(eff_label).and_then(|i| i.span);
            let mut diag = match (sev, proof) {
                (Severity::Error, Some(proof)) => Diagnostic::error(
                    "infinite-loop",
                    proof,
                    format!(
                        "this effect recreates object state {state} it depends on. \
                         Every run stores a fresh reference (`Object.is` always fails) \
                         and re-triggers itself: infinite render loop",
                        state = state_slot_name(state_label, &state_vals)
                    ),
                ),
                (Severity::Info, _) => Diagnostic::info(
                    "infinite-loop",
                    format!(
                        "this effect depends on object state but freshly recreates \
                         state {state} outside its deps. No update cycle was found, \
                         but deps may be too imprecise to rule one out",
                        state = state_slot_name(state_label, &state_vals)
                    ),
                ),
                _ => Diagnostic::warn(
                    "infinite-loop",
                    format!(
                        "this effect may store a fresh reference into state \
                         {state} which its deps react to: possible infinite render loop",
                        state = state_slot_name(state_label, &state_vals)
                    ),
                ),
            }
            .with_label(state_label);
            if let Some(r) = eff_span {
                diag = diag.with_range(r);
            }
            if let Some(r) = call_span {
                diag = diag.with_step(
                    crate::rules::Step::Write {
                        slot: state_label,
                        value: crate::rules::ValueClass::Fresh,
                    },
                    Some(*eff_label),
                    Some(r),
                    &|l| state_slot_name(l, &state_vals),
                );
            }
            diags.push(diag);
        }
    }
    diags
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::hooks::{DepsArg, DepsList};
    use crate::{
        domains::{StateValue, StateValueTransfer},
        engine::{AnalysisResult, Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };
    use std::collections::HashSet;

    fn prog(name: &str, r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        crate::test_support::prog(name, r.clone())
    }

    fn trivial_cfg() -> CFG {
        crate::test_support::single_block_cfg(vec![])
    }

    fn make_result_with_widened(
        widened: HashSet<usize>,
        hooks: Vec<HookEntry>,
        render_stmts: Vec<Stmt>,
    ) -> AnalysisResult<StateValue> {
        AnalysisResult {
            widen_trace: widened
                .into_iter()
                .map(|l| (l, crate::engine::WidenEvent::default()))
                .collect(),
            hooks,
            ..crate::test_support::analysis_result(crate::test_support::single_block_cfg(
                render_stmts,
            ))
        }
    }

    #[test]
    fn no_widened_labels_no_warning() {
        let result = make_result_with_widened(HashSet::new(), vec![], vec![]);
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn widened_with_unconditional_setter_warns() {
        let eff_cfg = crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit {
                    id: crate::ir::types::ExprId(0),
                    fields: vec![],
                }],
            },
            None,
        )]);

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: DepsArg::Absent,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];

        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    /// Issue #86: the churn graph is whole-program data. Checking N components
    /// must build it once — rebuilding it inside every `check` made the rules
    /// phase quadratic in component count (dub/twenty never finished).
    #[test]
    fn churn_graph_is_built_once_per_program() {
        use crate::rules::api::cache::ProgramCache;
        use crate::rules::helpers::churn_graph::BUILDS;

        let eff_cfg = crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit {
                    id: crate::ir::types::ExprId(0),
                    fields: vec![],
                }],
            },
            None,
        )]);
        let one = make_result_with_widened(
            HashSet::from([0]),
            vec![HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::Absent,
                span: None,
            }],
            vec![Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            }],
        );

        let names = ["A".to_string(), "B".to_string(), "C".to_string()];
        let mut program = prog(&names[0], &one);
        for n in &names[1..] {
            program.components.insert(n.clone(), one.clone());
        }

        BUILDS.with(|n| n.set(0));
        let cache = ProgramCache::new(&program);
        for n in &names {
            let diags = InfiniteLoop.check(&RuleCtx::cached(&cache, n, Default::default()));
            assert!(!diags.is_empty(), "the churn arm must actually run for {n}");
        }
        assert_eq!(
            BUILDS.with(|n| n.get()),
            1,
            "churn graph rebuilt per component (#86)"
        );
    }

    #[test]
    fn widened_but_setter_only_conditional_no_warning() {
        // Effect body has no setter call in entry block → no warning
        let eff_cfg = trivial_cfg(); // empty body
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: DepsArg::Absent,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn empty_deps_array_never_warns() {
        // deps: Some([]) = mount-only, no cycle even if setter called
        let eff_cfg = crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit {
                    id: crate::ir::types::ExprId(0),
                    fields: vec![],
                }],
            },
            None,
        )]);
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: DepsArg::List(DepsList::exact(vec![])),
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "deps:[] = one-shot, never infinite"
        );
    }

    #[test]
    fn widened_different_state_no_warning() {
        let eff_cfg = crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setOther".to_string())),
                args: vec![Expr::ObjectLit {
                    id: crate::ir::types::ExprId(0),
                    fields: vec![],
                }],
            },
            None,
        )]);

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: DepsArg::Absent,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn via_analyze_component_widening_threshold_1() {
        let eff_cfg = crate::test_support::single_block_cfg(vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::ObjectLit {
                        id: crate::ir::types::ExprId(0),
                        fields: vec![],
                    }],
                },
                None,
            ),
        ]);

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::Absent,
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        let comp = ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(render_stmts),
            hooks,
            hook_provenance: vec![],
            module_consts: Default::default(),
        };
        let config = Config {
            widen_threshold: 1,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));
        assert!(!diags.is_empty(), "expected InfiniteLoop warning");
    }

    #[test]
    fn count_plus_one_infinite_loop_detected() {
        // useEffect(() => { setCount(count + 1) }, [count]) count grows unboundedly
        let eff_cfg = crate::test_support::single_block_cfg(vec![
            Stmt::Let {
                var: "setCount".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setCount".to_string())),
                    args: vec![Expr::BinOp {
                        op: crate::ir::expr::BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            ),
        ]);

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "count".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: "setCount".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        let comp = ComponentIR {
            file: std::path::PathBuf::new(),
            name: "Counter".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(render_stmts),
            hooks,
            hook_provenance: vec![],
            module_consts: Default::default(),
        };
        let config = Config {
            widen_threshold: 3,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        assert!(result.widen_trace.contains_key(&0), "count should widen");
        let diags = InfiniteLoop.check(&RuleCtx::new(
            &prog("Counter", &result),
            &"Counter".to_string(),
        ));
        assert!(
            !diags.is_empty(),
            "setState(count+1) should be detected as infinite loop"
        );
    }

    #[test]
    fn setter_in_non_entry_block_still_warns() {
        // block 0 (empty entry) → jump → block 1 (has setter)
        let mut eff_blocks = std::collections::BTreeMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: crate::ir::cfg::Terminator::Jump(1),
            },
        );
        eff_blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::ObjectLit {
                            id: crate::ir::types::ExprId(0),
                            fields: vec![],
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![crate::ir::cfg::Edge {
                from: 0,
                to: 1,
                kind: crate::ir::cfg::EdgeKind::Unconditional,
            }],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: DepsArg::Absent,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));
        assert!(
            !diags.is_empty(),
            "setter in block 1 should be detected via BFS"
        );
    }

    // ── callback traversal ───────────────────────────────────────────────────

    /// Component with effect body `setX = StateSetter(0); call_expr`, state[0] init 0.
    fn component_with_effect_call(
        setter_name: &str,
        call_expr: Expr,
        deps: DepsArg,
    ) -> ComponentIR {
        let eff_cfg = crate::test_support::single_block_cfg(vec![
            Stmt::Let {
                var: setter_name.to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(call_expr, None),
        ]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps,
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: setter_name.to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(render_stmts),
            hooks,
            hook_provenance: vec![],
            module_consts: Default::default(),
        }
    }

    /// `() => setN(n + 1)` as a single-block FnLit.
    fn incrementing_setter_cb(setter_name: &str) -> Expr {
        Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: std::sync::Arc::new(crate::test_support::single_block_cfg(vec![
                Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var(setter_name.to_string())),
                        args: vec![Expr::BinOp {
                            op: crate::ir::expr::BinOp::Add,
                            lhs: Box::new(Expr::StateVal(0)),
                            rhs: Box::new(Expr::Lit(Prim::Int(1))),
                        }],
                    },
                    None,
                ),
            ])),
        }
    }

    #[test]
    fn then_callback_setter_triggers_infinite_loop() {
        // useEffect(() => { p.then(() => setN(n + 1)) }, [n])
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![incrementing_setter_cb("setN")],
        };
        let comp = component_with_effect_call(
            "setN",
            call,
            DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
        );
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widen_trace.contains_key(&0),
            "n should widen via the .then callback"
        );
        assert!(
            !InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "setN(n+1) inside .then should be detected as infinite loop"
        );
    }

    #[test]
    fn add_event_listener_setter_does_not_loop() {
        // useEffect(() => { el.addEventListener('click', () => setN(n + 1)) })
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("el".to_string())),
                field: "addEventListener".to_string(),
            }),
            args: vec![
                Expr::Lit(Prim::String("click".to_string())),
                incrementing_setter_cb("setN"),
            ],
        };
        let comp = component_with_effect_call("setN", call, DepsArg::Absent);
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            !result.widen_trace.contains_key(&0),
            "event handler must not widen state (would be a false positive)"
        );
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "addEventListener handler must not trigger InfiniteLoop"
        );
    }

    #[test]
    fn unknown_callee_setter_does_not_loop() {
        // useEffect(() => { myHelper(() => setN(n + 1)) }) unknown callee not descended
        let call = Expr::Call {
            fn_: Box::new(Expr::Var("myHelper".to_string())),
            args: vec![incrementing_setter_cb("setN")],
        };
        let comp = component_with_effect_call("setN", call, DepsArg::Absent);
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(!result.widen_trace.contains_key(&0));
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn back_edge_in_then_callback_now_detected() {
        // useEffect(() => { p.then(() => { loop { setN(n+1) } }) }) back edge in callback
        let mut cb_blocks = std::collections::BTreeMap::new();
        cb_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::BinOp {
                            op: crate::ir::expr::BinOp::Add,
                            lhs: Box::new(Expr::StateVal(0)),
                            rhs: Box::new(Expr::Lit(Prim::Int(1))),
                        }],
                    },
                    None,
                )],
                term: Terminator::Jump(0), // self-loop
            },
        );
        let cb = Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: std::sync::Arc::new(CFG {
                entry: 0,
                blocks: cb_blocks,
                edges: vec![crate::ir::cfg::Edge {
                    from: 0,
                    to: 0,
                    kind: crate::ir::cfg::EdgeKind::Back,
                }],
            }),
        };
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![cb],
        };
        let comp = component_with_effect_call("setN", call, DepsArg::Absent);
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widen_trace.contains_key(&0),
            "back-edge in callback body → side-effect traversal → setN fires → widening"
        );
        assert!(
            !InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "the loop setter in the .then callback must now be flagged"
        );
    }

    #[test]
    fn setter_in_loop_in_then_does_not_loop_when_bounded() {
        // useEffect(() => { p.then(() => { while (..) { setN(0) } }) }, [n]) constant setter stabilises
        let mut cb_blocks = std::collections::BTreeMap::new();
        cb_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(1),
            },
        );
        cb_blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 2,
                    else_: 3,
                },
            },
        );
        cb_blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::Lit(Prim::Int(0))],
                    },
                    None,
                )],
                term: Terminator::Jump(1), // back to header
            },
        );
        cb_blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cb = Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: std::sync::Arc::new(CFG {
                entry: 0,
                blocks: cb_blocks,
                edges: vec![
                    crate::ir::cfg::Edge {
                        from: 0,
                        to: 1,
                        kind: crate::ir::cfg::EdgeKind::Unconditional,
                    },
                    crate::ir::cfg::Edge {
                        from: 1,
                        to: 2,
                        kind: crate::ir::cfg::EdgeKind::IfTrue,
                    },
                    crate::ir::cfg::Edge {
                        from: 1,
                        to: 3,
                        kind: crate::ir::cfg::EdgeKind::IfFalse,
                    },
                    crate::ir::cfg::Edge {
                        from: 2,
                        to: 1,
                        kind: crate::ir::cfg::EdgeKind::Back,
                    },
                ],
            }),
        };
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![cb],
        };
        let comp = component_with_effect_call(
            "setN",
            call,
            DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
        );
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            !result.widen_trace.contains_key(&0),
            "bounded setter in a loop stabilises → must not widen"
        );
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "bounded loop setter must not be flagged (anti-FP)"
        );
    }

    /// Component with effect body built from `stmts`, state[0] init 0.
    fn component_with_effect_stmts(
        setter_name: &str,
        stmts: Vec<Stmt>,
        deps: DepsArg,
    ) -> ComponentIR {
        let mut eff_blocks = std::collections::BTreeMap::new();
        let mut all_stmts = vec![Stmt::Let {
            var: setter_name.to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        all_stmts.extend(stmts);
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: all_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps,
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: setter_name.to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(render_stmts),
            hooks,
            hook_provenance: vec![],
            module_consts: Default::default(),
        }
    }

    #[test]
    fn var_callback_setter_triggers_infinite_loop() {
        // const cb = () => setN(n + 1); setTimeout(cb, 1000)  deps: [n]
        use crate::ir::types::ExprId;
        let cb_body_cfg = {
            crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::BinOp {
                        op: crate::ir::expr::BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            )])
        };
        let stmts = vec![
            Stmt::Let {
                var: "cb".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(10),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(cb_body_cfg),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setTimeout".to_string())),
                    args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(1000))],
                },
                None,
            ),
        ];
        let comp = component_with_effect_stmts(
            "setN",
            stmts,
            DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
        );
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widen_trace.contains_key(&0),
            "n should widen via the variable callback"
        );
        assert!(
            !InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "setN(n+1) inside cb → setTimeout(cb) should be detected as infinite loop"
        );
    }

    #[test]
    fn var_callback_then_setter_triggers_infinite_loop() {
        // const inc = () => setN(n + 1); fetch().then(inc)  deps: [n]
        use crate::ir::types::ExprId;
        let cb_body_cfg = {
            crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::BinOp {
                        op: crate::ir::expr::BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            )])
        };
        let stmts = vec![
            Stmt::Let {
                var: "inc".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(11),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(cb_body_cfg),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::FieldAccess {
                        obj: Box::new(Expr::Call {
                            fn_: Box::new(Expr::Var("fetch".to_string())),
                            args: vec![],
                        }),
                        field: "then".to_string(),
                    }),
                    args: vec![Expr::Var("inc".to_string())],
                },
                None,
            ),
        ];
        let comp = component_with_effect_stmts(
            "setN",
            stmts,
            DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
        );
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widen_trace.contains_key(&0),
            "n should widen via the variable .then callback"
        );
        assert!(
            !InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "setN(n+1) inside inc → fetch().then(inc) should be detected as infinite loop"
        );
    }

    #[test]
    fn nested_var_callback_chain_triggers_infinite_loop() {
        // outer() → setTimeout(inner) → setN(n+1) deps: [n]
        use crate::ir::types::ExprId;
        let inner_body_cfg = {
            crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::BinOp {
                        op: crate::ir::expr::BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            )])
        };
        let outer_body_cfg = {
            crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setTimeout".to_string())),
                    args: vec![Expr::Var("inner".to_string()), Expr::Lit(Prim::Int(100))],
                },
                None,
            )])
        };
        let stmts = vec![
            Stmt::Let {
                var: "inner".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(50),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(inner_body_cfg),
                },
                span: None,
            },
            Stmt::Let {
                var: "outer".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(51),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(outer_body_cfg),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("outer".to_string())),
                    args: vec![],
                },
                None,
            ),
        ];
        let comp = component_with_effect_stmts(
            "setN",
            stmts,
            DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
        );
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widen_trace.contains_key(&0),
            "n should widen via B6→B5 nested chain"
        );
        assert!(
            !InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
                .is_empty(),
            "outer() → setTimeout(inner) → setN(n+1) should be detected as infinite loop"
        );
    }

    fn setter_cfg(setter_var: &str) -> CFG {
        crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var(setter_var.to_string())),
                args: vec![Expr::Lit(Prim::Int(1))],
            },
            None,
        )])
    }

    #[test]
    fn handler_note_attached_when_handler_calls_setter() {
        let hooks = vec![
            HookEntry::Effect {
                label: 1,
                body_cfg: setter_cfg("setN"),
                deps: DepsArg::Absent,
                span: None,
            },
            HookEntry::Handler {
                label: 2,
                event: "click".to_string(),
                body_cfg: setter_cfg("setN"),
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));

        assert!(!diags.is_empty(), "should detect infinite loop");
        let handler_notes: Vec<_> = diags[0]
            .notes
            .iter()
            .filter(|n| matches!(n.step, crate::rules::Step::Handler { .. }))
            .collect();
        assert_eq!(handler_notes.len(), 1, "one Handler step for the handler");
        assert_eq!(
            handler_notes[0].hook_label,
            Some(2),
            "note → handler label 2"
        );
        // The fixpoint evidence closes the chain (ADR-019).
        assert!(
            diags[0]
                .notes
                .iter()
                .any(|n| matches!(n.step, crate::rules::Step::Widen { slot: 0, .. })),
            "widen step present"
        );
    }

    #[test]
    fn no_note_when_no_handler_calls_setter() {
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: setter_cfg("setN"),
            deps: DepsArg::Absent,
            span: None,
        }];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));

        assert!(!diags.is_empty(), "should detect infinite loop");
        assert!(
            !diags[0]
                .notes
                .iter()
                .any(|n| matches!(n.step, crate::rules::Step::Handler { .. })),
            "no Handler step when no handler calls the setter"
        );
    }
}
