use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, StateValueTransfer, Transfer,
        impls::Stability,
        stores::{Heap, MemoStore, StateStore},
    },
    engine::ProgramAnalysisResult,
    ir::{
        SourceRange,
        cfg::CFG,
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use super::{
    Diagnostic, Rule, Severity, all_deps_unstable, collect_component_setter_vars,
    collect_fn_bindings, collect_setter_calls, collect_setter_calls_with_extra, memo_val_labels,
    resolve_setter_aliases, setter_var_labels, state_slot_name, state_val_labels,
};

/// Fires when an effect causes an infinite render loop.
///
/// - `"infinite-loop"` local state widens in fixpoint.
/// - `"cross-component-infinite-loop"` parent state widens via ComponentSetter prop.
///
/// Effects with `deps: []` (mount-only) are excluded. Effects with all-unstable deps
/// are treated as no-deps. If parent not in results, cross fires as Warning.
pub struct InfiniteLoop;

impl Rule for InfiniteLoop {
    fn name(&self) -> &'static str {
        "infinite-loop"
    }

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        use crate::engine::HookKind;
        // A render→effect→setState cycle needs both a state slot and an effect.
        (super::has_hook_kind(result, component, HookKind::State)
            && super::has_hook_kind(result, component, HookKind::Effect))
        .then_some(super::SafeCheck {
            rule: self.name(),
            message: "no effect diverges into an infinite render loop",
        })
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let comp_result = &result.components[component];

        let local_setter_labels: HashMap<Var, HookLabel> =
            setter_var_labels(&comp_result.render_cfg);
        let state_names = state_val_labels(&comp_result.render_cfg);

        // ComponentSetter props, excluding self-references.
        let cs_vars: HashMap<Var, (Symbol, HookLabel)> = collect_component_setter_vars(
            &comp_result.render_cfg,
            &comp_result.block_states,
            &comp_result.heap,
        )
        .into_iter()
        .filter(|(_, (parent_comp, _))| parent_comp != component)
        .collect();

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

            // Mount-only: fires once, no loop.
            if matches!(deps, Some(d) if d.is_empty()) {
                continue;
            }
            // Non-empty deps with at least one stable value genuinely gate the effect.
            if let Some(dep_exprs) = deps
                && !all_deps_unstable(dep_exprs, comp_result)
            {
                continue;
            }

            let calls =
                collect_setter_calls_with_extra(body_cfg, &all_setter_vars, 1, &render_fn_bindings);

            for call in &calls {
                if let Some(&state_label) = local_setter_labels.get(&call.var) {
                    // ── Intra ─────────────────────────────────────────────────
                    if !comp_result.widen_trace.contains_key(&state_label) {
                        continue; // state didn't diverge → bounded
                    }
                    let writes = comp_result.effect_setter_writes.get(state_label);
                    if !writes.is_bottom_value() && !writes.is_unbounded() {
                        continue; // write bounded → narrowing held the growth
                    }

                    let deps_note = if deps.is_some() {
                        " (all deps unstable effect runs every render)"
                    } else {
                        ""
                    };
                    let mut diag = Diagnostic::new(
                        "infinite-loop",
                        format!(
                            "this effect keeps pushing state {}{} to new values \
                             on every run potential infinite render loop",
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
                                super::Step::Handler {
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
                    diag = diag.with_notes(super::witness::slot_history(
                        comp_result,
                        state_label,
                        &|l| state_slot_name(l, &state_names),
                    ));

                    reported_effects.insert(*eff_label);
                    diags.push(diag);
                } else if let Some((parent_comp, parent_label)) = cs_vars.get(&call.var) {
                    // ── Cross-component ────────────────────────────────────────
                    let shared_write = result.shared_state.get(parent_comp, *parent_label);
                    if shared_write.is_bottom_value() {
                        continue; // setter not reached in semantic analysis
                    }
                    if !shared_write.is_unbounded() {
                        continue; // write is bounded → no divergence
                    }

                    let deps_note = if deps.is_some() {
                        " (all deps unstable effect runs every render)"
                    } else {
                        ""
                    };
                    let msg = format!(
                        "this effect calls `{}`, a state setter of parent `{}`{} \
                         parent re-renders → child re-renders → effect fires again: infinite loop",
                        call.var, parent_comp, deps_note
                    );
                    let mut diag = Diagnostic::new("cross-component-infinite-loop", msg)
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
        let (cycle_diags, covered) =
            check_multi_effect_cycles(result, component, &reported_effects);
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
    result: &ProgramAnalysisResult,
    component: &Symbol,
    reported_effects: &HashSet<HookLabel>,
) -> (Vec<Diagnostic>, HashSet<(HookLabel, HookLabel)>) {
    use super::churn_graph::{ChurnEdge, build_churn_graph, find_churn_cycles};

    let edges = build_churn_graph(result);
    if edges.is_empty() {
        return (Vec::new(), HashSet::new());
    }
    let cycles = find_churn_cycles(&edges);
    let comp_result = &result.components[component];

    let mut diags = Vec::new();
    let mut covered: HashSet<(HookLabel, HookLabel)> = HashSet::new();
    // Slot display names, resolved lazily per involved component.
    let mut names: HashMap<Symbol, HashMap<Var, HookLabel>> = HashMap::new();

    for cycle in &cycles {
        let cyc: Vec<&ChurnEdge> = cycle.edge_idx.iter().map(|&i| &edges[i]).collect();
        let path = {
            let mut parts: Vec<String> = cyc
                .iter()
                .map(|e| node_display(&e.from, component, result, &mut names))
                .collect();
            parts.push(node_display(&cyc[0].from, component, result, &mut names));
            parts.join(" → ")
        };
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
            // `Versioned`, never the exact slot) → Warning ceiling.
            let (rule, sev) = if cycle.cross_component {
                ("cross-component-infinite-loop", Severity::Warning)
            } else if cycle.all_must {
                ("infinite-loop", Severity::Error)
            } else {
                ("infinite-loop", Severity::Warning)
            };

            let to_name = node_display(&e.to, component, result, &mut names);
            let msg = if cyc.len() == 1 && e.no_deps {
                format!(
                    "this effect has no dependency array and stores a fresh \
                     reference into state {to_name} it re-runs after every \
                     render and re-triggers itself: infinite render loop"
                )
            } else if cyc.len() == 1 {
                format!(
                    "this effect stores a fresh reference into state {to_name} \
                     which its own deps react to the re-render runs it again: \
                     infinite render loop"
                )
            } else if cycle.all_must {
                format!(
                    "these effects form a state-update cycle ({path}) each \
                     step stores a fresh reference that re-runs the next \
                     effect: infinite render loop"
                )
            } else {
                format!(
                    "these effects may form a state-update cycle ({path}) \
                     each step may store a fresh reference that re-runs the \
                     next effect: possible infinite render loop"
                )
            };

            let mut diag = Diagnostic::new(rule, msg)
                .with_severity(sev)
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
                    super::Step::Write {
                        slot: e.to.1,
                        value: super::ValueClass::Fresh,
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
                    super::Step::CycleEdge {
                        from: other_from,
                        to: other_to,
                    },
                    Some(other.effect_label),
                    other.write_span,
                    &super::witness::fallback_name,
                );
            }
            diags.push(diag);
        }
    }
    (diags, covered)
}

/// Display name of a qualified slot: `` `count` `` locally,
/// `` `count` of `Parent` `` for another component's slot.
fn node_display(
    node: &super::churn_graph::SlotNode,
    component: &Symbol,
    result: &ProgramAnalysisResult,
    names: &mut HashMap<Symbol, HashMap<Var, HookLabel>>,
) -> String {
    let map = names.entry(node.0.clone()).or_insert_with(|| {
        result
            .components
            .get(&node.0)
            .map(|r| resolve_setter_aliases(&r.render_cfg, &state_val_labels(&r.render_cfg)))
            .unwrap_or_default()
    });
    let base = state_slot_name(node.1, map);
    if node.0 == *component {
        base
    } else {
        format!("{base} of `{}`", node.0)
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Freshness {
    Not,
    /// May store a fresh reference (opaque value, imprecise updater).
    Maybe,
    /// Must store a fresh reference every call (`PerRender` argument).
    Fresh,
}

/// A `setX(arg)` call site found in an effect body. The target slot is
/// qualified `(component, label)` so `ComponentSetter` props (writes into a
/// parent slot) are first-class alongside local setters.
pub(super) struct ChurnSetterCall {
    pub(super) node: super::churn_graph::SlotNode,
    pub(super) freshness: Freshness,
    /// Top-level block of the effect body; `None` when nested in a callback
    /// (then never "must-reached").
    pub(super) block_id: Option<BlockId>,
    pub(super) span: Option<SourceRange>,
    /// Abstract value being stored (fresh-reference approximation for
    /// functional updaters). Used for the convergence proof.
    pub(super) written: crate::domains::StateValue,
}

fn peel(mut e: &Expr) -> &Expr {
    while let Expr::TSAnnotated(inner, _) = e {
        e = inner;
    }
    e
}

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
    let setter_nodes: HashMap<Var, super::churn_graph::SlotNode> = setter_labels
        .iter()
        .map(|(v, l)| (v.clone(), (component.clone(), *l)))
        .collect();
    let render_fn_bindings = collect_fn_bindings(cfg);
    let mut diags = Vec::new();

    for hook in &comp_result.hooks {
        let HookEntry::Effect {
            label: eff_label,
            body_cfg,
            deps: Some(dep_exprs),
            ..
        } = hook
        else {
            continue;
        };
        if dep_exprs.is_empty() {
            continue; // mount-only
        }

        let (exact, versioned_qualified) =
            classify_effect_deps(dep_exprs, comp_result, &state_vals, &memo_vals);
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
        let mut best: HashMap<HookLabel, (Severity, Option<SourceRange>)> = HashMap::new();
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
                    comp_result,
                )
            {
                continue;
            }
            let sev = if exact.contains(&state_label) || versioned.contains(&state_label) {
                let fresh_blocks: HashSet<BlockId> = calls
                    .iter()
                    .filter(|c| c.node == call.node && c.freshness == Freshness::Fresh)
                    .filter_map(|c| c.block_id)
                    .collect();
                if exact.contains(&state_label)
                    && call.freshness == Freshness::Fresh
                    && !fresh_blocks.is_empty()
                    && on_all_paths(body_cfg, &fresh_blocks)
                {
                    Severity::Error
                } else {
                    Severity::Warning
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
                Severity::Info
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
            let pos = |s: Option<SourceRange>| s.map_or((u32::MAX, u32::MAX), |r| (r.line, r.col));
            let entry = best.entry(state_label).or_insert((sev, call.span));
            if rank(sev) > rank(entry.0)
                || (rank(sev) == rank(entry.0) && pos(call.span) < pos(entry.1))
            {
                *entry = (sev, call.span);
            }
        }

        for (state_label, (sev, call_span)) in best {
            let eff_span = comp_result.effect_info.get(eff_label).and_then(|i| i.span);
            let mut diag = match sev {
                Severity::Error => Diagnostic::new(
                    "infinite-loop",
                    format!(
                        "this effect recreates object state {state} it depends on \
                         every run stores a fresh reference (`Object.is` always fails) \
                         and re-triggers itself: infinite render loop",
                        state = state_slot_name(state_label, &state_vals)
                    ),
                ),
                Severity::Warning => Diagnostic::new(
                    "infinite-loop",
                    format!(
                        "this effect may store a fresh reference into state \
                         {state} which its deps react to possible infinite render loop",
                        state = state_slot_name(state_label, &state_vals)
                    ),
                ),
                Severity::Info => Diagnostic::new(
                    "infinite-loop",
                    format!(
                        "this effect depends on object state but freshly recreates \
                         state {state} outside its deps no update cycle was found, \
                         but deps may be too imprecise to rule one out",
                        state = state_slot_name(state_label, &state_vals)
                    ),
                ),
            }
            .with_severity(sev)
            .with_label(state_label);
            if let Some(r) = eff_span {
                diag = diag.with_range(r);
            }
            if let Some(r) = call_span {
                diag = diag.with_step(
                    super::Step::Write {
                        slot: state_label,
                        value: super::ValueClass::Fresh,
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

/// Classify effect deps against the component's own state slots:
/// - `exact` — deps that ARE a local state slot (`StateVal(l)` or a var
///   resolving to one): must-change whenever a fresh value is stored.
/// - `versioned` — qualified slots `(component, label)` that merely version
///   a dep (field reads, memo chains, props): may-change under a fresh set.
pub(super) fn classify_effect_deps(
    dep_exprs: &[Expr],
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    state_vals: &HashMap<Var, HookLabel>,
    memo_vals: &HashMap<Var, HookLabel>,
) -> (HashSet<HookLabel>, HashSet<super::churn_graph::SlotNode>) {
    let mut exact: HashSet<HookLabel> = HashSet::new();
    let mut versioned: HashSet<super::churn_graph::SlotNode> = HashSet::new();
    for dep in dep_exprs {
        match peel(dep) {
            Expr::StateVal(l) => {
                exact.insert(*l);
            }
            Expr::Var(v) if state_vals.contains_key(v) => {
                exact.insert(state_vals[v]);
            }
            other => {
                // Memo/callback bindings: their env value is stale ⊤
                // (bound before memo recompute) — read the memo store.
                let val = match other {
                    Expr::MemoVal(l) | Expr::CallbackVal(l) => comp_result.memo_store.get(*l),
                    Expr::Var(v) if memo_vals.contains_key(v) => {
                        comp_result.memo_store.get(memo_vals[v])
                    }
                    _ => eval_in_exit_env(other, comp_result),
                };
                if let Stability::Versioned(labels) = &val.reference {
                    for (c, l) in labels {
                        versioned.insert((c.clone(), *l));
                    }
                }
            }
        }
    }
    (exact, versioned)
}

/// Projection of a written value onto its reference slot — what a
/// reference-churn loop can actually carry across renders. Every primitive
/// part (which cannot fail `Object.is` freshly) is dropped, so guard proofs
/// don't lose to residual ⊤ noise. A ⊥ reference slot yields ⊥: no
/// reference can ever be stored → the claimed reference churn is vacuous.
pub(super) fn reference_part(written: &crate::domains::StateValue) -> crate::domains::StateValue {
    crate::domains::StateValue::reference(written.reference.clone())
}

/// Evaluate `expr` in the render exit environment (same pattern as
/// `all_deps_unstable`).
pub(super) fn eval_in_exit_env(
    expr: &Expr,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> crate::domains::StateValue {
    let exit_env = comp_result.exit_env();
    let mut s: StateStore<crate::domains::StateValue> = comp_result.state_store.clone();
    let mut m: MemoStore<crate::domains::StateValue> = comp_result.memo_store.clone();
    let mut h = Heap::new();
    StateValueTransfer.eval_expr(
        expr,
        &exit_env,
        &mut AnalysisCtx::null(comp_result.component.clone(), &mut s, &mut m, &mut h),
    )
}

/// Must the argument of a setter call store a fresh reference?
fn arg_freshness(
    arg: &Expr,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> Freshness {
    match peel(arg) {
        // Functional updater: React stores the *return* value.
        Expr::FnLit {
            params, body_cfg, ..
        } => {
            let mut returns = Vec::new();
            for block in body_cfg.blocks.values() {
                if let crate::ir::cfg::Terminator::Return(e) = &block.term {
                    returns.push(peel(e));
                }
            }
            if returns.is_empty() {
                return Freshness::Maybe;
            }
            let fresh = returns
                .iter()
                .map(|e| classify_updater_return(e, params))
                .collect::<Vec<_>>();
            if fresh.iter().all(|f| *f == Freshness::Fresh) {
                Freshness::Fresh
            } else if fresh.iter().all(|f| *f == Freshness::Not) {
                Freshness::Not
            } else {
                Freshness::Maybe
            }
        }
        other => {
            // Churn is about the REFERENCE kind only: a widened numeric value
            // (`count + 1`) changes but never fails `Object.is` freshly.
            let val = eval_in_exit_env(other, comp_result);
            match &val.reference {
                Stability::PerRender => {
                    if val.is_unstable_reference_only() {
                        Freshness::Fresh
                    } else {
                        Freshness::Maybe // joined with other kinds
                    }
                }
                Stability::Unknown => Freshness::Maybe,
                // Stable / Versioned / ⊥ reference; residual ⊤ stays Maybe.
                _ if val.other => Freshness::Maybe,
                _ => Freshness::Not,
            }
        }
    }
}

/// Freshness of one return expression of a functional updater, without an
/// environment (the updater runs in its own scope).
fn classify_updater_return(e: &Expr, params: &[Var]) -> Freshness {
    match peel(e) {
        Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::FnLit { .. } => Freshness::Fresh,
        // Identity updater `o => o` and literal resets converge.
        Expr::Var(v) if params.first().is_some_and(|p| p == v) => Freshness::Not,
        Expr::Lit(_) => Freshness::Not,
        // JS operators return primitives — except logical ops, which return
        // an operand: never *must*-fresh, at most maybe.
        Expr::BinOp { lhs, rhs, .. } => {
            let l = classify_updater_return(lhs, params);
            let r = classify_updater_return(rhs, params);
            l.max(r).min(Freshness::Maybe)
        }
        Expr::UnaryOp { .. } => Freshness::Not,
        _ => Freshness::Maybe,
    }
}

/// Recursively collect `setX(arg)` calls with their argument freshness.
/// `top_level` — block IDs belong to the effect body CFG (must-reach usable).
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_churn_calls(
    cfg: &CFG,
    setter_nodes: &HashMap<Var, super::churn_graph::SlotNode>,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    depth: usize,
    top_level: bool,
    out: &mut Vec<ChurnSetterCall>,
) {
    let mut local_bindings = collect_fn_bindings(cfg);
    for (k, v) in fn_bindings {
        local_bindings
            .entry(k.clone())
            .or_insert_with(|| Arc::clone(v));
    }
    for block in cfg.blocks.values() {
        let block_id = if top_level { Some(block.id) } else { None };
        for stmt in &block.stmts {
            let (expr, span) = match stmt {
                Stmt::ExprStmt(e, span) => (e, *span),
                Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => (rhs, None),
            };
            churn_calls_in_expr(
                expr,
                span,
                block_id,
                setter_nodes,
                &local_bindings,
                comp_result,
                depth,
                out,
            );
        }
        match &block.term {
            crate::ir::cfg::Terminator::Return(e)
            | crate::ir::cfg::Terminator::Branch { cond: e, .. } => {
                churn_calls_in_expr(
                    e,
                    None,
                    block_id,
                    setter_nodes,
                    &local_bindings,
                    comp_result,
                    depth,
                    out,
                );
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn churn_calls_in_expr(
    expr: &Expr,
    span: Option<SourceRange>,
    block_id: Option<BlockId>,
    setter_nodes: &HashMap<Var, super::churn_graph::SlotNode>,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    depth: usize,
    out: &mut Vec<ChurnSetterCall>,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Expr::Var(name) = peel(fn_) {
                if let Some(node) = setter_nodes.get(name) {
                    let freshness = args
                        .first()
                        .map(|a| arg_freshness(a, comp_result))
                        .unwrap_or(Freshness::Not);
                    let written = match args.first().map(peel) {
                        // A fresh-returning updater stores a fresh (truthy,
                        // non-null) reference — enough for guard proofs.
                        Some(Expr::FnLit { .. }) => {
                            crate::domains::StateValue::reference(Stability::PerRender)
                        }
                        Some(a) => eval_in_exit_env(a, comp_result),
                        None => crate::domains::StateValue::top(),
                    };
                    out.push(ChurnSetterCall {
                        node: node.clone(),
                        freshness,
                        block_id,
                        span,
                        written,
                    });
                } else if depth > 0
                    && let Some(body) = fn_bindings.get(name)
                {
                    // Direct call of a bound helper: executes inline, keep
                    // the caller's block for must-reach (mirrors B6).
                    let mut inner = Vec::new();
                    collect_churn_calls(
                        body,
                        setter_nodes,
                        fn_bindings,
                        comp_result,
                        depth - 1,
                        false,
                        &mut inner,
                    );
                    for mut c in inner {
                        c.block_id = block_id;
                        c.span = c.span.or(span);
                        out.push(c);
                    }
                }
            }
            for arg in args {
                match arg {
                    // Callback passed elsewhere: runs at an unknown time,
                    // never must-reached.
                    Expr::FnLit { body_cfg, .. } if depth > 0 => {
                        collect_churn_calls(
                            body_cfg,
                            setter_nodes,
                            fn_bindings,
                            comp_result,
                            depth - 1,
                            false,
                            out,
                        );
                    }
                    _ => churn_calls_in_expr(
                        arg,
                        span,
                        block_id,
                        setter_nodes,
                        fn_bindings,
                        comp_result,
                        depth,
                        out,
                    ),
                }
            }
        }
        // Bare FnLit: body is a CFG, not a child expr — only runs if invoked
        // (covered by the Call arm above). Everything else: generic descent.
        other => {
            other.for_each_child(&mut |c| {
                churn_calls_in_expr(
                    c,
                    span,
                    block_id,
                    setter_nodes,
                    fn_bindings,
                    comp_result,
                    depth,
                    out,
                )
            });
        }
    }
}

/// True when the dominating guards of `call_block` provably kill the call
/// once `written` sits in state slot `label` — the set fires at most once.
///
/// Walks the single-predecessor chain up from the call block collecting
/// `(cond, taken)` branch constraints, rebinds every var aliasing the slot to
/// `written`, and applies the engine's branch narrowing: if the guarded
/// variable narrows to ⊥, the branch is dead in every later run.
pub(super) fn converges_once_written(
    cfg: &CFG,
    call_block: BlockId,
    state_vals: &HashMap<Var, HookLabel>,
    label: HookLabel,
    written: &crate::domains::StateValue,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> bool {
    use crate::ir::cfg::{EdgeKind, Terminator};

    let mut guards: Vec<(&Expr, bool)> = Vec::new();
    let mut cur = call_block;
    loop {
        let preds: Vec<&crate::ir::cfg::Edge> = cfg.edges.iter().filter(|e| e.to == cur).collect();
        if preds.len() != 1 {
            break; // join point or entry: stop collecting dominators
        }
        let edge = preds[0];
        if let Some(pb) = cfg.blocks.get(&edge.from)
            && let Terminator::Branch { cond, .. } = &pb.term
        {
            match edge.kind {
                EdgeKind::IfTrue => guards.push((cond, true)),
                EdgeKind::IfFalse => guards.push((cond, false)),
                _ => {}
            }
        }
        cur = edge.from;
        if cur == cfg.entry {
            break;
        }
    }
    if guards.is_empty() {
        return false;
    }

    // Compound booleans (`a || b`, `a && b`) lower to a short-circuit temp
    // (`__tN`) branched on directly — narrowing `__tN` alone proves nothing
    // about the slot read inside an operand. Expand each guard into the
    // conjunctive facts it implies over the operands.
    let mut conjuncts: Vec<(&Expr, bool)> = Vec::new();
    for (cond, taken) in guards {
        expand_guard(cfg, cond, taken, 4, &mut conjuncts);
    }

    let mut env = comp_result.exit_env();
    for (v, l) in state_vals {
        if *l == label {
            env.extend(v.clone(), written.clone());
        }
    }
    for (cond, taken) in conjuncts {
        let narrowed = crate::engine::cfg_analyzer::narrow_env_for_branch(&env, cond, taken);
        if let Some(x) = guard_var(cond)
            && narrowed.lookup(x).is_bottom_value()
        {
            return true;
        }
        env = narrowed;
    }
    false
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
    use crate::ir::cfg::{EdgeKind, Terminator};

    if depth == 0 {
        out.push((cond, taken));
        return;
    }
    match cond {
        // `!e` flips the polarity of `e`.
        Expr::UnaryOp {
            op: crate::ir::expr::UnaryOp::Not,
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
                    Some(Terminator::Branch { cond: c, then_, else_ })
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
            op: crate::ir::expr::UnaryOp::Not,
            arg,
        } => match arg.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

/// True when every entry→exit path of `cfg` passes through one of `blocks`.
pub(super) fn on_all_paths(cfg: &CFG, blocks: &HashSet<BlockId>) -> bool {
    if blocks.contains(&cfg.entry) {
        return true;
    }
    // BFS avoiding `blocks`; reaching an exit block means a path escapes.
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue = vec![cfg.entry];
    visited.insert(cfg.entry);
    while let Some(bid) = queue.pop() {
        let succs = cfg.successors(bid);
        if succs.is_empty() {
            return false; // exit reached without hitting a call block
        }
        for succ in succs {
            if !blocks.contains(&succ) && visited.insert(succ) {
                queue.push(succ);
            }
        }
    }
    true
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            StateValue, StateValueTransfer,
            stores::{MemoStore, StateStore},
        },
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
    use std::collections::{HashMap, HashSet};

    fn prog(name: &str, r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        use crate::domains::stores::SharedStateStore;
        use crate::engine::program_result::{AnalysisStats, ComponentCallGraph};
        let mut components = HashMap::new();
        components.insert(name.to_string(), r.clone());
        ProgramAnalysisResult {
            components,
            shared_state: SharedStateStore::default(),
            call_graph: ComponentCallGraph::new(),
            recursive_components: HashSet::new(),
            stats: AnalysisStats::default(),
            file_table: Default::default(),
            function_registry: Default::default(),
        }
    }

    fn trivial_cfg() -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    fn make_result_with_widened(
        widened: HashSet<usize>,
        hooks: Vec<HookEntry>,
        render_stmts: Vec<Stmt>,
    ) -> AnalysisResult<StateValue> {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        AnalysisResult {
            component: "C".to_string(),
            file: Default::default(),
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widen_trace: widened
                .into_iter()
                .map(|l| (l, crate::engine::WidenEvent::default()))
                .collect(),
            inline_origins: Vec::new(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            iterations: 0,
            effect_setter_writes: StateStore::bottom(),
            heap: crate::domains::stores::Heap::new(),
        }
    }

    #[test]
    fn no_widened_labels_no_warning() {
        let result = make_result_with_widened(HashSet::new(), vec![], vec![]);
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn widened_with_unconditional_setter_warns() {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
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
            edges: vec![],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];

        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn widened_but_setter_only_conditional_no_warning() {
        // Effect body has no setter call in entry block → no warning
        let eff_cfg = trivial_cfg(); // empty body
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn empty_deps_array_never_warns() {
        // deps: Some([]) = mount-only, no cycle even if setter called
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
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
            edges: vec![],
        };
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: Some(vec![]),
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "deps:[] = one-shot, never infinite"
        );
    }

    #[test]
    fn widened_different_state_no_warning() {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setOther".to_string())),
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
            edges: vec![],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn via_analyze_component_widening_threshold_1() {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
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
                ],
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
                deps: None,
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
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let comp = ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        };
        let config = Config {
            widen_threshold: 1,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());
        assert!(!diags.is_empty(), "expected InfiniteLoop warning");
    }

    #[test]
    fn count_plus_one_infinite_loop_detected() {
        // useEffect(() => { setCount(count + 1) }, [count]) count grows unboundedly
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
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
                ],
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
                deps: Some(vec![Expr::StateVal(0)]),
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
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let comp = ComponentIR {
            file: std::path::PathBuf::new(),
            name: "Counter".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        };
        let config = Config {
            widen_threshold: 3,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        assert!(result.widen_trace.contains_key(&0), "count should widen");
        let diags = InfiniteLoop.check(&prog("Counter", &result), &"Counter".to_string());
        assert!(
            !diags.is_empty(),
            "setState(count+1) should be detected as infinite loop"
        );
    }

    #[test]
    fn setter_in_non_entry_block_still_warns() {
        // block 0 (empty entry) → jump → block 1 (has setter)
        let mut eff_blocks = HashMap::new();
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
            deps: None,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());
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
        deps: Option<Vec<Expr>>,
    ) -> ComponentIR {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
                    Stmt::Let {
                        var: setter_name.to_string(),
                        rhs: Expr::StateSetter(0),
                        span: None,
                    },
                    Stmt::ExprStmt(call_expr, None),
                ],
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
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        }
    }

    /// `() => setN(n + 1)` as a single-block FnLit.
    fn incrementing_setter_cb(setter_name: &str) -> Expr {
        let mut b = HashMap::new();
        b.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var(setter_name.to_string())),
                        args: vec![Expr::BinOp {
                            op: crate::ir::expr::BinOp::Add,
                            lhs: Box::new(Expr::StateVal(0)),
                            rhs: Box::new(Expr::Lit(Prim::Int(1))),
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: std::sync::Arc::new(CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }),
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
        let comp = component_with_effect_call("setN", call, Some(vec![Expr::StateVal(0)]));
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
                .check(&prog("C", &result), &"C".to_string())
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
        let comp = component_with_effect_call("setN", call, None);
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
                .check(&prog("C", &result), &"C".to_string())
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
        let comp = component_with_effect_call("setN", call, None);
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn back_edge_in_then_callback_now_detected() {
        // useEffect(() => { p.then(() => { loop { setN(n+1) } }) }) back edge in callback
        let mut cb_blocks = HashMap::new();
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
        let comp = component_with_effect_call("setN", call, None);
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "the loop setter in the .then callback must now be flagged"
        );
    }

    #[test]
    fn setter_in_loop_in_then_does_not_loop_when_bounded() {
        // useEffect(() => { p.then(() => { while (..) { setN(0) } }) }, [n]) constant setter stabilises
        let mut cb_blocks = HashMap::new();
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
        let comp = component_with_effect_call("setN", call, Some(vec![Expr::StateVal(0)]));
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "bounded loop setter must not be flagged (anti-FP)"
        );
    }

    /// Component with effect body built from `stmts`, state[0] init 0.
    fn component_with_effect_stmts(
        setter_name: &str,
        stmts: Vec<Stmt>,
        deps: Option<Vec<Expr>>,
    ) -> ComponentIR {
        let mut eff_blocks = HashMap::new();
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
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        }
    }

    #[test]
    fn var_callback_setter_triggers_infinite_loop() {
        // const cb = () => setN(n + 1); setTimeout(cb, 1000)  deps: [n]
        use crate::ir::types::ExprId;
        let cb_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
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
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
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
        let comp = component_with_effect_stmts("setN", stmts, Some(vec![Expr::StateVal(0)]));
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "setN(n+1) inside cb → setTimeout(cb) should be detected as infinite loop"
        );
    }

    #[test]
    fn var_callback_then_setter_triggers_infinite_loop() {
        // const inc = () => setN(n + 1); fetch().then(inc)  deps: [n]
        use crate::ir::types::ExprId;
        let cb_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
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
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
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
        let comp = component_with_effect_stmts("setN", stmts, Some(vec![Expr::StateVal(0)]));
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "setN(n+1) inside inc → fetch().then(inc) should be detected as infinite loop"
        );
    }

    #[test]
    fn nested_var_callback_chain_triggers_infinite_loop() {
        // outer() → setTimeout(inner) → setN(n+1) deps: [n]
        use crate::ir::types::ExprId;
        let inner_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
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
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
        };
        let outer_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
                0,
                BasicBlock {
                    id: 0,
                    stmts: vec![Stmt::ExprStmt(
                        Expr::Call {
                            fn_: Box::new(Expr::Var("setTimeout".to_string())),
                            args: vec![Expr::Var("inner".to_string()), Expr::Lit(Prim::Int(100))],
                        },
                        None,
                    )],
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
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
        let comp = component_with_effect_stmts("setN", stmts, Some(vec![Expr::StateVal(0)]));
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
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "outer() → setTimeout(inner) → setN(n+1) should be detected as infinite loop"
        );
    }

    fn setter_cfg(setter_var: &str) -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var(setter_var.to_string())),
                        args: vec![Expr::Lit(Prim::Int(1))],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    #[test]
    fn handler_note_attached_when_handler_calls_setter() {
        let hooks = vec![
            HookEntry::Effect {
                label: 1,
                body_cfg: setter_cfg("setN"),
                deps: None,
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
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());

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
            deps: None,
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
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());

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
