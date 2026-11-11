use crate::rules::RuleCtx;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    engine::ProgramAnalysisResult,
    ir::{
        SourceRange,
        cfg::CFG,
        hooks::HookEntry,
        types::{BlockId, HookLabel, Var},
    },
};

use crate::engine::setters::SetterCallPhase;
use crate::engine::{
    Always, Change, ChurnEdge, ChurnGraph, EdgeStrength, EveryRender, Rerun, WriterRegion,
};
use crate::ir::ComponentId;
use crate::rules::api::query::must_effect_cycle;
use crate::rules::helpers::cycles::{NodeNames, cycle_path, node_display};
use crate::rules::{
    Certified, Diagnostic, MustResult, OnAllPaths, Rule, Severity, all_deps_provably_stable,
    all_setter_labels, collect_fn_bindings, collect_setter_calls_with_extra, must_on_all_paths,
    resolve_setter_aliases, state_slot_name, state_val_labels,
};

/// Fires when an effect causes an infinite render loop.
///
/// - `"infinite-loop"` an effect's write re-triggers it, alone or in a cycle
///   of effects: the churn graph's arms, which claim a loop unless they prove
///   the write settles (#265 retired the arm that read a widened slot).
/// - `"cross-component-infinite-loop"` parent state widens via ComponentSetter
///   prop, or a churn cycle crosses components.
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
        let comp_result = &result.components[&component];

        // Authoritative setter map: base setters plus alias chains
        // (`const s1 = setX`, `s2 = s1`) resolved across the render and every
        // hook body — a setter called in an effect through an alias must still
        // be attributed to its state slot (else a real loop goes unflagged).
        let local_setter_labels = all_setter_labels(comp_result);

        // ComponentSetter props, excluding self-references.
        let cs_vars: HashMap<Var, Vec<crate::engine::setters::SetterProp>> =
            crate::rules::cross_component_setters(comp_result, component);

        // No early return when this is empty: the churn arms below read the
        // graph, whose rows also write through setters only a value holds
        // (`t[1]` of a `useState` result kept whole, #283).
        let mut all_setter_vars: HashSet<Var> = local_setter_labels.names().cloned().collect();
        all_setter_vars.extend(cs_vars.keys().cloned());

        let render_fn_bindings: HashMap<Var, Arc<CFG>> =
            collect_fn_bindings(&comp_result.render_cfg);
        let mut diags = Vec::new();
        // Effects already reported by the cross arm — the churn cycle arm
        // skips them (same outage class, one report per effect).
        let mut reported_effects: HashSet<HookLabel> = HashSet::new();
        // The graph is whole-program data: read it from the ctx cache, which
        // builds it once for the run (issue #86).
        let graph = ctx.cache().churn();

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
            // is empty says so (the re-run class).
            if comp_result.rerun(*eff_label) == Some(Rerun::MountOnly) {
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
                collect_setter_calls_with_extra(body_cfg, &all_setter_vars, &render_fn_bindings);

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
                // A write of this component's own slot is the churn graph's,
                // whose arms below claim a loop unless they prove it settles
                // (#265): what remains here is a parent's setter, reached
                // through a prop.
                if local_setter_labels.targets(&call.var).next().is_some() {
                    continue;
                }
                let Some(props) = cs_vars.get(&call.var) else {
                    continue;
                };
                // ── Cross-component ────────────────────────────────────────────
                // A prop several call sites bind to several parents'
                // setters: one finding per parent a slot of which diverges.
                let mut named: HashSet<ComponentId> = HashSet::new();
                for prop in props {
                    let (parent_comp, parent_label) = (prop.component, prop.label);
                    if named.contains(&parent_comp) {
                        continue;
                    }
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
                        ctx.source_names().name(&call.var),
                        result.display_name(parent_comp),
                        deps_note
                    );
                    let mut diag = Diagnostic::warn("cross-component-infinite-loop", msg)
                        .with_label(*eff_label);

                    if let Some(r) = comp_result.effect_info.get(eff_label).and_then(|i| i.span) {
                        diag = diag.with_range(r);
                    }
                    named.insert(parent_comp);
                    reported_effects.insert(*eff_label);
                    diags.push(diag);
                }
            }
        }

        // ── F5b: multi-effect churn cycles (see churn_graph.rs) ───────────────
        let (cycle_diags, covered) =
            check_multi_effect_cycles(ctx, graph, result, component, &reported_effects);
        diags.extend(cycle_diags);
        // Self-churn arm last: its Info branch skips writes a cycle covers.
        diags.extend(check_object_churn(
            ctx,
            graph,
            result,
            component,
            &covered,
            &reported_effects,
        ));

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
    ctx: &RuleCtx,
    graph: &ChurnGraph,
    result: &ProgramAnalysisResult,
    component: ComponentId,
    reported_effects: &HashSet<HookLabel>,
) -> (Vec<Diagnostic>, HashSet<(HookLabel, HookLabel)>) {
    let (edges, cycles) = (&graph.edges, &graph.cycles);
    if edges.is_empty() {
        return (Vec::new(), HashSet::new());
    }
    let comp_result = &result.components[&component];

    let mut diags = Vec::new();
    let mut covered: HashSet<(HookLabel, HookLabel)> = HashSet::new();
    let mut cycle_reported: HashSet<HookLabel> = HashSet::new();
    // Slot display names, resolved lazily per involved component.
    let mut names: NodeNames = HashMap::new();

    for cycle in cycles {
        let cyc: Vec<&ChurnEdge> = cycle.edge_idx.iter().map(|&i| &edges[i]).collect();
        let path = cycle_path(edges, cycle, component, result, &mut names);
        for e in &cyc {
            if e.component != component {
                continue;
            }
            if e.to.0 == component {
                covered.insert((e.effect_label, e.to.1));
            }
            if reported_effects.contains(&e.effect_label) {
                continue; // the cross arm already flagged this effect
            }
            // One report per effect: a swap's two writes are two edges of one
            // cycle in one effect. Cycles come must-first, so the kept report
            // is the strongest.
            if !cycle_reported.insert(e.effect_label) {
                continue;
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

            // The wording follows the proof, not `all_must`: a cross-component
            // cycle can be all-must and still carry no proof (Warning).
            let certain = cycle_proof.is_some();
            let to_name = node_display(&e.to, component, result, &mut names);
            // A step that may only change a primitive is not a fresh reference.
            let allocates = |e: &ChurnEdge| {
                result.components[&e.component]
                    .slot_writers
                    .iter()
                    .any(|w| {
                        w.span == e.write_span
                            && w.region == WriterRegion::Effect(e.effect_label)
                            && w.written.change == Change::Must
                    })
            };
            // Why a length-1 cycle re-runs its own effect after every render.
            let always = e
                .always
                .as_ref()
                .filter(|_| cyc.len() == 1)
                .map(|a| always_clause(a, result, e.component, e.effect_label));
            let msg = if !certain && !cyc.iter().all(|e| allocates(e)) {
                if let Some((why, reruns)) = &always {
                    format!(
                        "this effect {why} and may change state {to_name}, so it \
                         {reruns} after every render and no write of it is proven to \
                         settle: possible infinite render loop"
                    )
                } else if cyc.len() == 1 {
                    format!(
                        "this effect may change state {to_name} which its own deps \
                         react to, and no write of it is proven to settle: possible \
                         infinite render loop"
                    )
                } else {
                    format!(
                        "these effects may form a state-update cycle ({path}) where \
                         no step is proven to settle: possible infinite render loop"
                    )
                }
            } else if let (Some((why, _)), true) = (&always, certain) {
                proven_self_loop(e.stepping, &to_name, Some(why))
            } else if let Some((why, reruns)) = &always {
                format!(
                    "this effect {why} and may store a fresh reference into state \
                     {to_name}, so it {reruns} after every render and can \
                     re-trigger itself: possible infinite render loop"
                )
            } else if cyc.len() == 1 && certain && e.stepping {
                proven_self_loop(true, &to_name, None)
            } else if cyc.len() == 1 && certain {
                format!(
                    "this effect stores a fresh reference into state {to_name} \
                     which its own deps react to, so the re-render runs it again: \
                     infinite render loop"
                )
            } else if cyc.len() == 1 {
                format!(
                    "this effect may store a fresh reference into state {to_name} \
                     which its own deps react to, so the re-render can run it again: \
                     possible infinite render loop"
                )
            } else if certain && cyc.iter().any(|e| e.stepping) {
                format!(
                    "these effects form a state-update cycle ({path}) where each \
                     step changes the slot the next effect re-runs on, at every \
                     run: infinite render loop"
                )
            } else if certain {
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
                        value: if allocates(e) {
                            crate::rules::ValueClass::Fresh
                        } else {
                            crate::rules::ValueClass::Unknown
                        },
                    },
                    Some(e.effect_label),
                    Some(r),
                    // Qualified display (may name a parent component's slot).
                    &ctx.names(&|_| to_name.clone()),
                );
            }
            // Point at the cycle's other steps living in this component.
            for other in &cyc {
                if other.effect_label == e.effect_label || other.component != component {
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
                    &ctx.names(&crate::rules::api::witness::fallback_name),
                );
            }
            diags.push(diag);
        }
    }
    (diags, covered)
}

/// Why effect `label` re-runs after every render, as a clause of the
/// message, and whether it surely does (`re-runs`) or only may (`may re-run`)
/// — `surely` as the engine computed it on the edge — naming the parents
/// whose elements alone make it so (#237).
fn always_clause(
    always: &EveryRender,
    program: &ProgramAnalysisResult,
    component: ComponentId,
    label: HookLabel,
) -> (String, &'static str) {
    let comp = &program.components[&component];
    let (why, surely) = (always.why, always.surely);
    let dep = |i: usize| {
        comp.hooks
            .iter()
            .find_map(|h| match h {
                HookEntry::Effect { label: l, deps, .. } if *l == label => deps
                    .list()
                    .and_then(|d| d.as_slice().get(i))
                    .map(|e| e.describe()),
                _ => None,
            })
            .unwrap_or_else(|| "a dep".to_string())
    };
    let clause = match why {
        Always::NoDeps => "has no dependency array".to_string(),
        Always::FreshDep(i) if surely => {
            format!("depends on `{}`, a new reference every render,", dep(i))
        }
        // Fresh under these parents' elements alone.
        Always::FreshDep(i) if !always.through.is_empty() => format!(
            "depends on `{}`, a new reference whenever {} renders it,",
            dep(i),
            crate::rules::parent_elements(program, &always.through)
        ),
        // Fresh while the props hold: a prop the parent allocates.
        Always::FreshDep(i) => format!(
            "depends on `{}`, a new reference whenever its parent re-renders,",
            dep(i)
        ),
        Always::OpaqueDeps => "is gated by a deps argument the analysis cannot read".to_string(),
        Always::MutableDep(i) => format!(
            "depends on `{}`, which may differ at any render without a state change,",
            dep(i)
        ),
    };
    (clause, if surely { "re-runs" } else { "may re-run" })
}

/// The message of a proven self-loop, built here for whichever arm reports
/// its edge (#280): an effect whose write changes the slot it re-runs on at
/// every run, by a new reference or by a step from a bounded start
/// (`ChurnEdge::stepping`). `why` says why an effect re-runs after every
/// render; `None` is one that depends on the slot.
fn proven_self_loop(stepping: bool, state: &str, why: Option<&str>) -> String {
    match (stepping, why) {
        (false, None) => format!(
            "this effect recreates object state {state} it depends on. Every run stores a \
             fresh reference (`Object.is` always fails) and re-triggers itself: infinite \
             render loop"
        ),
        (false, Some(why)) => format!(
            "this effect {why} and stores a fresh reference into state {state}, so it \
             re-runs after every render and re-triggers itself: infinite render loop"
        ),
        (true, None) => format!(
            "this effect moves state {state} it depends on by a step from a bounded start, \
             so every run changes it and re-triggers itself: infinite render loop"
        ),
        (true, Some(why)) => format!(
            "this effect {why} and moves state {state} by a step from a bounded start, so \
             it re-runs after every render and re-triggers itself: infinite render loop"
        ),
    }
}

// ── Object-churn arm (ADR-017) ────────────────────────────────────────────────
//
// `useEffect(() => setObj({...obj}), [obj])` never widens: the reference slot
// converges (`join(PerRender, PerRender)`), so no widened value says it loops.
// Certainty comes from *dep structure* instead: a dep that is the state
// slot itself must-changes when its setter stores a must-fresh reference.
//
// Stratification (Error = all-must, per the diagnostic doctrine):
// - Error:   dep is slot X exactly ∧ setX(fresh, or a step from a bounded
//            start, #280) on ALL paths of the body
// - Warning: dep versioned by X (alias/memo/field) ∧ body may-call setX(fresh?)
// - Info:    effect deps on object state but freshly sets a DIFFERENT object
//            state — cross-effect cycles are not analyzed (FN-flavor limit)

fn check_object_churn(
    ctx: &RuleCtx,
    graph: &ChurnGraph,
    result: &ProgramAnalysisResult,
    component: ComponentId,
    covered: &HashSet<(HookLabel, HookLabel)>,
    reported_effects: &HashSet<HookLabel>,
) -> Vec<Diagnostic> {
    let comp_result = &result.components[&component];
    let cfg = &comp_result.render_cfg;
    let state_vals = resolve_setter_aliases(cfg, &state_val_labels(cfg));
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
        if deps.list().is_none() {
            continue; // no readable dependency array: the graph arm's self-edge
        }

        // This arm's partition of the relation (ADR-020 item 2, ADR-042 §4):
        // the effect's dep-driven edges from this component's own slots into
        // this component's own slots. A `self_slot` edge is the loop; every
        // other one feeds the Info branch. The change leg, the convergence kill and
        // the field-sensitive re-trigger test (#90) were applied by the engine
        // when it built the edge. An edge an every-render re-run carries is
        // the graph arm's, like the self-edge of an effect with no array.
        let edges = graph.edges.iter().filter(|e| {
            e.component == component
                && e.effect_label == *eff_label
                && e.always.is_none()
                && e.from.0 == component
                && e.to.0 == component
        });

        // Strongest verdict per state label.
        // Best churn finding per slot: (severity, call span, Error proof,
        // whether the effect stores a new reference into the slot, whether
        // the proven loop rests on a step instead).
        type ChurnBest = (
            Severity,
            Option<SourceRange>,
            Option<Certified<OnAllPaths>>,
            bool,
            bool,
        );
        let mut best: HashMap<HookLabel, ChurnBest> = HashMap::new();
        for e in edges {
            let state_label = e.to.1;
            let rows = || {
                comp_result
                    .slot_writers
                    .iter()
                    .filter(|w| w.owner.is_none() && w.slot == state_label)
                    .filter(|w| w.region == WriterRegion::Effect(*eff_label))
            };
            let allocates = rows().any(|w| w.written.change == Change::Must);
            let (sev, churn_proof) = if e.self_slot {
                // The Error anchors on the changing write being on all paths
                // — a new reference, or a step when the edge rests on one —
                // re-derived from the writer rows and minted by the
                // must-primitive, never trusted from the edge's own strength.
                let changing_blocks: HashSet<BlockId> = rows()
                    .filter(|w| match e.stepping {
                        true => w.written.step.is_some(),
                        false => w.written.change == Change::Must,
                    })
                    .filter_map(|w| w.block)
                    .collect();
                if e.strength == EdgeStrength::Must && !changing_blocks.is_empty() {
                    match must_on_all_paths(body_cfg, &changing_blocks) {
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
            // Rank ties break on earliest source position, so the row a
            // finding anchors on does not depend on relation order.
            let pos = |s: Option<SourceRange>| s.map_or((u32::MAX, u32::MAX), |r| r.pos_key());
            let replace = match best.get(&state_label) {
                None => true,
                Some(entry) => {
                    rank(sev) > rank(entry.0)
                        || (rank(sev) == rank(entry.0) && pos(e.write_span) < pos(entry.1))
                }
            };
            if replace {
                best.insert(
                    state_label,
                    (sev, e.write_span, churn_proof, allocates, e.stepping),
                );
            }
        }

        for (state_label, (sev, call_span, proof, allocates, stepping)) in best {
            // One report per effect, as in the cycle arm: an effect the
            // cross arm flagged keeps that report, unless this arm holds the
            // stronger, proven verdict.
            if reported_effects.contains(eff_label) && sev != Severity::Error {
                continue;
            }
            let eff_span = comp_result.effect_info.get(eff_label).and_then(|i| i.span);
            let mut diag = match (sev, proof) {
                (Severity::Error, Some(proof)) => Diagnostic::error(
                    "infinite-loop",
                    proof,
                    proven_self_loop(
                        stepping,
                        &state_slot_name(state_label, ctx.source_names(), &state_vals),
                        None,
                    ),
                ),
                (Severity::Info, _) if allocates => Diagnostic::info(
                    "infinite-loop",
                    format!(
                        "this effect depends on object state but freshly recreates \
                         state {state} outside its deps. No update cycle was found, \
                         but deps may be too imprecise to rule one out",
                        state = state_slot_name(state_label, ctx.source_names(), &state_vals)
                    ),
                ),
                (Severity::Info, _) => Diagnostic::info(
                    "infinite-loop",
                    format!(
                        "this effect depends on state and may change state {state} \
                         outside its deps. No update cycle was found, but deps may be \
                         too imprecise to rule one out",
                        state = state_slot_name(state_label, ctx.source_names(), &state_vals)
                    ),
                ),
                _ if allocates => Diagnostic::warn(
                    "infinite-loop",
                    format!(
                        "this effect may store a fresh reference into state \
                         {state} which its deps react to: possible infinite render loop",
                        state = state_slot_name(state_label, ctx.source_names(), &state_vals)
                    ),
                ),
                _ => Diagnostic::warn(
                    "infinite-loop",
                    format!(
                        "this effect may change state {state} which its deps react to, \
                         and no write of it is proven to settle: possible infinite \
                         render loop",
                        state = state_slot_name(state_label, ctx.source_names(), &state_vals)
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
                        value: if allocates {
                            crate::rules::ValueClass::Fresh
                        } else {
                            crate::rules::ValueClass::Unknown
                        },
                    },
                    Some(*eff_label),
                    Some(r),
                    &ctx.names(&|l| state_slot_name(l, ctx.source_names(), &state_vals)),
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

    fn prog(name: &str, r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        crate::test_support::prog(name, r.clone())
    }

    fn trivial_cfg() -> CFG {
        crate::test_support::single_block_cfg(vec![])
    }

    /// `setN({})` as a statement.
    fn fresh_object_write() -> Expr {
        Expr::Call {
            fn_: Box::new(Expr::Var("setN".to_string())),
            args: vec![Expr::ObjectLit {
                id: crate::ir::types::ExprId(0),
                fields: vec![],
            }],
        }
    }

    fn loops_of(result: &AnalysisResult<StateValue>) -> Vec<Diagnostic> {
        InfiniteLoop.check(&RuleCtx::new(&prog("C", result), result.component))
    }

    #[test]
    fn no_effect_no_warning() {
        let result = crate::test_support::analysis_result(trivial_cfg());
        assert!(
            InfiniteLoop
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
                .is_empty()
        );
    }

    /// An effect with no deps array storing a fresh object at every run: the
    /// churn graph's self-edge, proven (#265: it used to be the widening
    /// arm's, on a hand-widened slot).
    #[test]
    fn an_unconditional_fresh_write_with_no_deps_is_a_proven_loop() {
        let result = analyze_component(
            component_with_effect_call("setN", fresh_object_write(), DepsArg::Absent),
            &StateValueTransfer,
            &Config::default(),
        );
        let diags = loops_of(&result);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].severity(), Severity::Error, "{diags:?}");
    }

    /// Issue #86: the churn graph is whole-program data. Checking N components
    /// must build it once — rebuilding it inside every `check` made the rules
    /// phase quadratic in component count (dub/twenty never finished).
    #[test]
    fn churn_graph_is_built_once_per_program() {
        use crate::engine::churn::BUILDS;
        use crate::rules::api::cache::ProgramCache;

        let one = analyze_component(
            component_with_effect_call("setN", fresh_object_write(), DepsArg::Absent),
            &StateValueTransfer,
            &Config::default(),
        );

        let ids = ["A", "B", "C"].map(crate::test_support::named);
        let mut program = prog("A", &one);
        for id in &ids {
            program.components.insert(*id, one.clone());
        }

        BUILDS.with(|n| n.set(0));
        let cache = ProgramCache::new(&program);
        for id in &ids {
            let diags = InfiniteLoop.check(&RuleCtx::cached(&cache, *id, Default::default()));
            assert!(
                !diags.is_empty(),
                "the churn arm must actually run for {id:?}"
            );
        }
        assert_eq!(
            BUILDS.with(|n| n.get()),
            1,
            "churn graph rebuilt per component (#86)"
        );
    }

    #[test]
    fn an_effect_with_no_setter_call_no_warning() {
        let result = analyze_component(
            component_with_effect(trivial_cfg(), DepsArg::Absent),
            &StateValueTransfer,
            &Config::default(),
        );
        assert!(loops_of(&result).is_empty());
    }

    #[test]
    fn empty_deps_array_never_warns() {
        // deps: Some([]) = mount-only, no cycle even if setter called
        let result = analyze_component(
            component_with_effect_call(
                "setN",
                fresh_object_write(),
                DepsArg::List(DepsList::exact(vec![])),
            ),
            &StateValueTransfer,
            &Config::default(),
        );
        assert!(
            loops_of(&result).is_empty(),
            "deps:[] = one-shot, never infinite"
        );
    }

    #[test]
    fn a_call_of_another_name_no_warning() {
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
        let result = analyze_component(
            component_with_effect(eff_cfg, DepsArg::Absent),
            &StateValueTransfer,
            &Config::default(),
        );
        assert!(loops_of(&result).is_empty());
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
                reducer: None,
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
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("C", &result), crate::test_support::C));
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
                reducer: None,
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
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("Counter", &result), result.component));
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
                term: Terminator::Return(Expr::Lit(Prim::Unit), None),
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

        let result = analyze_component(
            component_with_effect(eff_cfg, DepsArg::Absent),
            &StateValueTransfer,
            &Config::default(),
        );
        assert!(
            !loops_of(&result).is_empty(),
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
        component_with_setter_effect(setter_name, eff_cfg, deps)
    }

    /// Component `C` with state 0 (`n`, init 0, setter `setN`) and one
    /// effect (label 1) running `eff_cfg`.
    fn component_with_effect(eff_cfg: CFG, deps: DepsArg) -> ComponentIR {
        component_with_setter_effect("setN", eff_cfg, deps)
    }

    fn component_with_setter_effect(setter_name: &str, eff_cfg: CFG, deps: DepsArg) -> ComponentIR {
        let hooks = vec![
            HookEntry::State {
                label: 0,
                reducer: None,
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
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
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
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
                .is_empty(),
            "addEventListener handler must not trigger InfiniteLoop"
        );
    }

    #[test]
    fn unknown_callee_setter_may_loop() {
        // useEffect(() => { myHelper(() => setN(n + 1)) }): the closure
        // escapes into the unknown callee and runs with the handlers (#254).
        // Its write moves `n` but is no in-cycle growth; `myHelper` may call
        // it at once, so the loop is possible: a Warning.
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
        let diags = InfiniteLoop.check(&RuleCtx::new(&prog("C", &result), crate::test_support::C));
        assert!(
            !diags.is_empty() && diags.iter().all(|d| d.severity() == Severity::Warning),
            "{diags:?}"
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
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
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
                term: Terminator::Return(Expr::Lit(Prim::Unit), None),
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
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
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
                term: Terminator::Return(Expr::Lit(Prim::Unit), None),
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
                reducer: None,
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
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
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
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
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
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
                .is_empty(),
            "outer() → setTimeout(inner) → setN(n+1) should be detected as infinite loop"
        );
    }

    /// `setN(n + 1)`.
    fn growing_write() -> Expr {
        Expr::Call {
            fn_: Box::new(Expr::Var("setN".to_string())),
            args: vec![Expr::BinOp {
                op: crate::ir::expr::BinOp::Add,
                lhs: Box::new(Expr::StateVal(0)),
                rhs: Box::new(Expr::Lit(Prim::Int(1))),
            }],
        }
    }

    /// A no-deps effect that grows its slot loops, and a click handler that
    /// writes the slot too changes nothing: the churn graph claims the loop
    /// either way. These were the widening arm's handler-note tests (#265),
    /// run on a constant write a hand-widened slot made look like growth; the
    /// note went with the arm.
    #[test]
    fn a_growing_no_deps_write_loops_with_or_without_a_handler_writing_it() {
        for with_handler in [false, true] {
            let mut comp = component_with_effect_call("setN", growing_write(), DepsArg::Absent);
            if with_handler {
                comp.hooks.push(HookEntry::Handler {
                    label: 2,
                    event: "click".to_string(),
                    body_cfg: crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
                        growing_write(),
                        None,
                    )]),
                    span: None,
                    dom_event: true,
                });
            }
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            assert!(
                !loops_of(&result).is_empty(),
                "with a handler: {with_handler}"
            );
        }
    }
}
