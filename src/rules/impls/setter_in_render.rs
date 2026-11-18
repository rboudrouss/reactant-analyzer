use crate::rules::RuleCtx;
use std::collections::{HashMap, HashSet};

use crate::ir::{
    ComponentId,
    expr::Expr,
    stmt::Stmt,
    types::{BlockId, HookLabel, Var},
};

use crate::engine::guards::converges_once_written;
use crate::engine::triggers::{Always, Rerun};
use crate::engine::{Change, SlotWriter, WriterPhase, WriterRegion, triggers_of};
use crate::ir::hooks::HookEntry;
use crate::rules::api::query::must_render_write;
use crate::rules::helpers::eval_in_exit_env;
use crate::rules::{
    Diagnostic, ExitDominance, MustResult, Rule, Severity, must_alias_scopes,
    must_dominates_all_exits, must_state_vals,
};

/// Fires when a state setter is called directly in the render body either a
/// local setter (`StateSetter`) or a parent-component setter passed as a prop.
/// A `useMemo` body runs during render too, and its writes are reported with
/// the memo's re-run class (#249).
///
/// Rule names emitted in diagnostics:
/// - `"setter-in-render"`        local `useState` setter called in render.
/// - `"cross-setter-in-render"`  parent's setter (prop) called in render.
///
/// Severity:
/// - `Error`   call block dominates all render exits (unconditional).
/// - `Warning` conditional path or nested FnLit (dominance unknowable).
///
/// Silence: the sanctioned "adjust state during render" idiom — a
/// **conditional local** call whose dominating guards read the slot it
/// writes, such that once the written value sits in the slot the guard is
/// dead (`if (changed && open) setOpen(false)`). React explicitly allows
/// this shape (one convergent extra render, no loop); warning on it is
/// noise. Cross-component calls are never silenced — setting another
/// component's state during render is a runtime error regardless of guards.
pub struct SetterInRender;

impl SetterInRender {
    const NAME: &'static str = "setter-in-render";
}

impl Rule for SetterInRender {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        use crate::engine::HookKind;
        // Local setters exist iff the component has a useState slot.
        crate::rules::has_hook_kind(result, component, HookKind::State).then_some(
            crate::rules::SafeCheck {
                rule: Self::NAME,
                message: "no setter call during render fails to settle",
            },
        )
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let comp_result = &ctx.program().components[&ctx.component()];
        // Dominance over exits, built once (not per call × exit). A call block
        // dominating every exit ⟹ it runs unconditionally ⟹ the certified Error;
        // otherwise conditional ⟹ Warning.
        let exit_dom = ExitDominance::of(&comp_result.render_cfg);
        let mut out = self.render_writes(ctx, &exit_dom);
        out.extend(self.memo_writes(ctx, &exit_dom));
        out
    }
}

impl SetterInRender {
    /// The writes the render body runs, as the writer relation reads them off
    /// the call relation (ADR-047): one row per (call site, slot), the rows
    /// the Tier-A `render_setter_calls` relation lists too (#281). A write
    /// through a local setter, an alias, a stored pair, a parent's prop or a
    /// closure the render runs is the relation's answer, never a name table.
    fn render_writes(&self, ctx: &RuleCtx, exit_dom: &ExitDominance) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let comp_result = &result.components[&component];
        let names = ctx.source_names();
        let rows: Vec<&SlotWriter> = render_rows(comp_result).collect();
        if rows.is_empty() {
            return vec![];
        }

        // The bodies a binder in refuses a render name's must-alias. The
        // convergence proof rebinds a name to the written value, which claims
        // the name IS the slot: the must-alias, never the may table (#231).
        let scopes = must_alias_scopes(&comp_result.render_cfg, &comp_result.hooks);
        let state_vals = crate::engine::guards::must_slot_paths(
            &comp_result.render_cfg,
            &scopes,
            &must_state_vals(&comp_result.render_cfg, &scopes),
        );
        let members = crate::engine::guards::MemberWrites::of(comp_result);
        // The names some body of the component assigns, made once and only
        // when a closure's settling proof needs them.
        let assigned = std::cell::OnceCell::new();
        let all = rows.clone();
        let judge = |w: &SlotWriter| -> Option<Diagnostic> {
            let sync = w.phase == WriterPhase::Render;
            // Unconditional write ⟹ a `Certified` proof — the only path to
            // Error. The row is placed only where it runs synchronously in
            // the render, at least once per pass of its block, through a call
            // the relation is sure writes that one slot (ADR-047, design-walk
            // §3); its block must dominate every exit (#279). A ⊤ row is
            // never certain, whatever block it sits in.
            let proof = match sync.then_some(w.placed).flatten() {
                Some(block) => match must_render_write(exit_dom, block) {
                    MustResult::All(c) => Some(c),
                    _ => None,
                },
                None => None,
            };
            // Sanctioned adjust-during-render idiom: a conditional local
            // write whose guards die once the written value is in the slot
            // converges after one extra render. Only for the slot's sole
            // render-pass write: with two, the next render reads whichever
            // ran last. The guards are the write's own: the render's, or
            // those of a closure the render runs synchronously
            // (`[1].forEach(() => { if (n !== 1) setN(1); })`, #269).
            if proof.is_none()
                && sync
                && w.owner.is_none()
                && let Some(arg) = w.written.expr.as_ref()
                && let Some((site, bid)) = w.own
                && sole_render_writer(result, component, w.slot)
            {
                let settles = match site {
                    None => converges_once_written(
                        &comp_result.render_cfg,
                        bid,
                        &state_vals,
                        &members,
                        w.slot,
                        &eval_in_exit_env(arg, comp_result),
                        Some(arg),
                        &comp_result.exit_env(),
                        &mut |e| eval_in_exit_env(e, comp_result),
                    ),
                    Some(site) => settles_in_closure(
                        comp_result,
                        (site, bid),
                        arg,
                        w.slot,
                        &state_vals,
                        &members,
                        assigned.get_or_init(|| assigned_names(comp_result)),
                    ),
                };
                if settles {
                    return None;
                }
            }
            // One name at one site that writes several slots (`[setN,
            // setM].forEach((set) => set(x))`) names each by its slot's own
            // name: a message naming both `set` would say nothing of which.
            // An expression a message prints whole (`(a ? setN : setM)(1)`)
            // says it.
            let shared = !crate::ir::names::is_named_temp(&w.setter)
                && all.iter().any(|o| {
                    o.span == w.span
                        && o.setter == w.setter
                        && (o.owner, o.slot) != (w.owner, w.slot)
                });
            let slot_name = || {
                let slot = (w.owner.unwrap_or(component), w.slot);
                comp_result.calls.slot_name(slot).cloned()
            };
            let shown = match shared.then(slot_name).flatten() {
                Some(name) => names.name(&name).into_owned(),
                None => names.name(&w.setter).into_owned(),
            };
            let (rule_name, message) = match w.owner {
                None if sync => (
                    "setter-in-render",
                    format!(
                        "setter `{shown}` called directly in the render body, \
                         move this call into a useEffect or an event handler"
                    ),
                ),
                None => ("setter-in-render", unknown_phase_message(names, &w.setter)),
                Some(parent) if sync => (
                    "cross-setter-in-render",
                    format!(
                        "prop `{shown}` (a state setter of parent `{}`) called during render \
                         of `{}`, which triggers a parent re-render on every render",
                        result.display_name(parent),
                        result.display_name(component)
                    ),
                ),
                Some(parent) => (
                    "cross-setter-in-render",
                    format!(
                        "prop `{shown}` (a state setter of parent `{}`) is handed to a callee \
                         with no timing summary. If that callee runs it during render, \
                         `{}` re-renders its parent on every render",
                        result.display_name(parent),
                        result.display_name(component)
                    ),
                ),
            };
            let mut d = match proof {
                Some(proof) => Diagnostic::error(rule_name, proof, message),
                None => Diagnostic::warn(rule_name, message),
            };
            d = match w.owner {
                None => d.with_label(w.slot),
                Some(_) => d.with_var(w.setter.clone()),
            };
            if let Some(r) = w.span {
                d = d.with_range(r);
            }
            // Witness (ADR-019): the render-time setter call itself.
            d = d.with_step(
                crate::rules::Step::Call {
                    callee: w.setter.clone(),
                    class: crate::rules::EffectClass::Setter,
                },
                None,
                w.span,
                &ctx.names(&crate::rules::api::witness::fallback_name),
            );
            Some(d)
        };
        // Every site is judged: a setter whose render-pass site settles may
        // also be handed to a callee that runs it at any time. A slot is one
        // finding, its strongest site nothing clears (#285): a certified one
        // if any — `if (c) setN(1); setN(2)` loops on every render whatever
        // `c` — else the first, the synchronous ones first. First in the
        // source, not in the walk (#301).
        let mut rows = rows;
        rows.sort_by_key(|w| {
            (
                w.phase != WriterPhase::Render,
                w.span.is_none(),
                w.span.map(|r| (r.file, r.pos_key())),
            )
        });
        let mut out: Vec<((Option<ComponentId>, HookLabel), Diagnostic)> = Vec::new();
        for w in rows {
            let key = (w.owner, w.slot);
            let Some(d) = judge(w) else {
                continue;
            };
            match out.iter_mut().find(|(k, _)| *k == key) {
                None => out.push((key, d)),
                Some((_, kept))
                    if d.severity() == Severity::Error && kept.severity() != Severity::Error =>
                {
                    *kept = d;
                }
                Some(_) => {}
            }
        }
        out.into_iter().map(|(_, d)| d).collect()
    }

    /// The writes of a `useMemo` body (#249): it runs during render, each time
    /// the memo recomputes, so its writes are render-phase writes. Read off the
    /// writer relation, whose `Memo` rows are the body's synchronous writes.
    ///
    /// Error takes every conjunct: the memo is called on every render, it
    /// recomputes at every render — no deps list, or the written slot is an
    /// exact dep that every render-phase write of it moves (a new reference;
    /// a later `setS(s)` in the same pass undoes it) — and the write runs on
    /// every path of the body, through a name that is surely the setter
    /// (`block`). An empty list runs the body once per mount: one update,
    /// which settles. Anything else is a possible loop.
    fn memo_writes(&self, ctx: &RuleCtx, exit_dom: &ExitDominance) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let comp_result = &result.components[&component];
        let names = ctx.source_names();
        // The writes of this component's slots that may run in the render
        // pass, where the last one of a slot is what the re-render reads.
        let render_phase = |w: &&SlotWriter| {
            w.owner.is_none()
                && matches!(w.region, WriterRegion::Render | WriterRegion::Memo(_))
                && matches!(
                    w.phase,
                    WriterPhase::Render | WriterPhase::Memo | WriterPhase::Unknown
                )
        };
        comp_result
            .slot_writers
            .iter()
            .filter_map(|w| {
                // A write of the store is no setter's (ADR-046): the churn
                // graph carries it.
                let WriterRegion::Memo(memo) = w.region else {
                    return None;
                };
                if w.writes_store() {
                    return None;
                }
                // A deferred or handler row runs on another turn; ⊤ may run
                // in the memo's own run.
                let sync = match w.phase {
                    WriterPhase::Memo => true,
                    WriterPhase::Unknown => false,
                    _ => return None,
                };
                let rerun = comp_result.rerun(memo);
                if rerun == Some(Rerun::MountOnly) {
                    return None;
                }
                let body = comp_result.hooks.iter().find_map(|h| match h {
                    HookEntry::Memo {
                        label, body_cfg, ..
                    } if *label == memo => Some(body_cfg),
                    _ => None,
                })?;
                let every_render = rerun == Some(Rerun::Always(Always::NoDeps))
                    || (w.owner.is_none()
                        && triggers_of(&comp_result.effect_triggers, memo)
                            .any(|t| t.exact && t.slot == (component, w.slot))
                        && comp_result
                            .slot_writers
                            .iter()
                            .filter(render_phase)
                            .filter(|o| o.slot == w.slot)
                            .all(|o| o.written.change == Change::Must));
                let called = comp_result
                    .hook_calls
                    .iter()
                    .find(|c| c.label == memo)
                    .is_some_and(|c| matches!(exit_dom.certify(c.block_id), MustResult::All(_)));
                let proof = (sync && every_render && called)
                    .then_some(w.block)
                    .flatten()
                    .and_then(|b| match must_dominates_all_exits(body, b) {
                        MustResult::All(c) => Some(c),
                        _ => None,
                    });
                let (rule_name, message) = match w.owner {
                    None if sync => (
                        "setter-in-render",
                        format!(
                            "setter `{}` called in a useMemo body, which runs during render: \
                             move this call into a useEffect or an event handler",
                            names.name(&w.setter)
                        ),
                    ),
                    None => ("setter-in-render", unknown_phase_message(names, &w.setter)),
                    Some(parent) => (
                        "cross-setter-in-render",
                        format!(
                            "prop `{}` (a state setter of parent `{}`) called in a useMemo body \
                             during render of `{}`, which triggers a parent re-render",
                            names.name(&w.setter),
                            result.display_name(parent),
                            result.display_name(component)
                        ),
                    ),
                };
                let mut d = match proof {
                    Some(proof) => Diagnostic::error(rule_name, proof, message),
                    None => Diagnostic::warn(rule_name, message),
                };
                d = match w.owner {
                    None => d.with_label(w.slot),
                    Some(_) => d.with_var(w.setter.clone()),
                };
                if let Some(r) = w.span {
                    d = d.with_range(r);
                }
                d = d.with_step(
                    crate::rules::Step::Call {
                        callee: w.setter.clone(),
                        class: crate::rules::EffectClass::Setter,
                    },
                    None,
                    w.span,
                    &ctx.names(&crate::rules::api::witness::fallback_name),
                );
                Some(d)
            })
            .collect()
    }
}

/// The `Unknown`-phase wording. A ⊤ row is not a call in the render body: the
/// setter reached a callee the analyzer has no summary for (`handleSubmit(cb)`,
/// `composeEventHandlers(a, cb)`), so every phase is possible and the render
/// pass is one of them. Saying "called directly in the render body" of that row
/// would state as fact the one thing the walk could not establish.
fn unknown_phase_message(names: &crate::ir::SourceNames, var: &Var) -> String {
    format!(
        "setter `{}` is handed to a callee with no timing summary. If that callee \
         runs it during render, this re-renders on every render",
        names.name(var)
    )
}

/// The rows of the writer relation this rule judges in the render: the
/// render's own writes of a component's slot that may run in its pass. A
/// `Handler`, `Deferred` or `Cleanup` row is a *proof* of no — a user event
/// or a known deferring registrar stands between the render pass and the
/// write. `Unknown` is ⊤ and stays: ⊤ includes the render pass (#130),
/// except a JSX prop's, ⊤ only because the program dispatches events
/// (#219): this rule reads it as the event handler it is, as it always did
/// (`docs/limitations.md`).
pub(crate) fn render_rows(
    comp: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> impl Iterator<Item = &SlotWriter> {
    comp.slot_writers.iter().filter(|w| {
        w.region == WriterRegion::Render
            && !w.writes_store()
            && match w.phase {
                WriterPhase::Render => true,
                WriterPhase::Unknown => !w.listener,
                _ => false,
            }
    })
}

/// Every name some body of `comp` assigns (`x = …`): the render, its hook
/// bodies, every function nested in either.
fn assigned_names(
    comp: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> HashSet<Var> {
    let mut out = HashSet::new();
    let roots =
        std::iter::once(&comp.render_cfg).chain(comp.hooks.iter().filter_map(HookEntry::body_cfg));
    for root in roots {
        let nested = crate::ir::bindings::nested_bodies(root);
        for body in std::iter::once(root).chain(nested.into_iter().map(|(_, b)| b)) {
            for stmt in body.blocks.values().flat_map(|b| &b.stmts) {
                if let Stmt::Assign { var, .. } = stmt {
                    out.insert(var.clone());
                }
            }
        }
    }
    out
}

/// The write storing `arg` into slot `label` at block `block` of the closure
/// at `site`, which the render runs synchronously, settles: its guards there
/// die once the value sits in the slot ([`converges_once_written`], #269).
///
/// The closure runs at its call, in the render, so what it captures is what
/// the render's names hold there. The env is the render's exit env, read only
/// where it is that: a name bound once and never assigned holds one value per
/// render (#314.2: a capture assigned between the closure's definition and
/// its call holds neither value), and the closure's own names — its
/// parameters, its locals — are its own. Every other name is ⊤, and so is
/// the slot path a name the closure rebinds would claim.
#[allow(clippy::too_many_arguments)]
fn settles_in_closure(
    comp: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    (site, block): (crate::ir::types::ExprId, BlockId),
    arg: &Expr,
    label: HookLabel,
    slots: &HashMap<Var, crate::engine::guards::SlotPath>,
    members: &crate::engine::guards::MemberWrites<'_>,
    assigned: &HashSet<Var>,
) -> bool {
    let Some(body) = comp.calls.body(site) else {
        return false;
    };
    // Its parameters: the literal's, where the component writes it.
    let roots =
        std::iter::once(&comp.render_cfg).chain(comp.hooks.iter().filter_map(HookEntry::body_cfg));
    let Some(params) = roots
        .flat_map(crate::ir::bindings::nested_bodies)
        .find(|(_, b)| std::ptr::eq(*b, &**body))
        .map(|(params, _)| params)
    else {
        return false;
    };
    let own: HashSet<&str> = crate::ir::bindings::own_names(params, body).collect();
    let hidden = |v: &str| own.contains(v) || assigned.contains(v);
    let mut env = comp.exit_env();
    let mut read = HashSet::new();
    for b in body.blocks.values() {
        b.for_each_expr(&mut |e| crate::ir::free_vars::collect_used_vars(e, &mut read));
    }
    crate::ir::free_vars::collect_used_vars(arg, &mut read);
    read.extend(own.iter().map(|v| v.to_string()));
    for v in read.iter().filter(|v| hidden(v)) {
        env.forget(v);
    }
    let slots: HashMap<Var, crate::engine::guards::SlotPath> = slots
        .iter()
        .filter(|(v, _)| !hidden(v))
        .map(|(v, p)| (v.clone(), p.clone()))
        .collect();
    let mut heap = comp.heap.clone();
    let mut eval = |e: &Expr| {
        crate::engine::eval::eval_in_stores(
            e,
            &env,
            comp.component,
            &comp.state_store,
            &comp.memo_store,
            &mut heap,
        )
    };
    let written = eval(arg);
    converges_once_written(
        body,
        block,
        &slots,
        members,
        label,
        &written,
        Some(arg),
        &env,
        &mut eval,
    )
}

/// Slot `label` of `component` has exactly one write that may run in a
/// render pass — of its own render or memo bodies, or of another component's
/// (a prop) — in the writer relation.
fn sole_render_writer(
    result: &crate::engine::ProgramAnalysisResult,
    component: crate::ir::ComponentId,
    label: HookLabel,
) -> bool {
    let render_pass = |w: &&SlotWriter| {
        w.slot == label
            && matches!(w.region, WriterRegion::Render | WriterRegion::Memo(_))
            && matches!(
                w.phase,
                WriterPhase::Render | WriterPhase::Memo | WriterPhase::Unknown
            )
    };
    let own = result.components[&component]
        .slot_writers
        .iter()
        .filter(|w| w.owner.is_none())
        .filter(render_pass)
        .count();
    let others = result
        .components
        .iter()
        .filter(|(c, _)| **c != component)
        .flat_map(|(_, r)| &r.slot_writers)
        .filter(|w| w.owner == Some(component))
        .filter(render_pass)
        .count();
    own == 1 && others == 0
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{StateValue, StateValueTransfer},
        engine::{AnalysisResult, Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{BinOp, Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::{Rule, Severity},
    };

    fn prog(name: &str, r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        crate::test_support::prog(name, r.clone())
    }

    fn make_result(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> AnalysisResult<StateValue> {
        AnalysisResult {
            hooks,
            ..crate::test_support::analysis_result(crate::test_support::single_block_cfg(
                render_stmts,
            ))
        }
    }

    #[test]
    fn no_setter_no_warning() {
        let result = make_result(vec![], vec![]);
        assert!(
            SetterInRender
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
                .is_empty()
        );
    }

    #[test]
    fn setter_not_called_no_warning() {
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result(vec![], render_stmts);
        assert!(
            SetterInRender
                .check(&RuleCtx::new(&prog("C", &result), crate::test_support::C))
                .is_empty()
        );
    }

    #[test]
    fn via_analyze_component_count_plus_one_is_error() {
        // setCount(count + 1) directly in render body → Error (unconditional).
        let hooks = vec![HookEntry::State {
            label: 0,
            reducer: None,
            init: Expr::Lit(Prim::Int(0)),
            span: None,
        }];
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
            Stmt::ExprStmt(
                Expr::Call {
                    id: crate::ir::expr::SYNTHETIC_CALL,
                    fn_: Box::new(Expr::Var("setCount".to_string())),
                    args: vec![Expr::BinOp {
                        op: BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            ),
        ];
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit), None),
            },
        );
        let comp = ComponentIR {
            file: std::path::PathBuf::new(),
            name: "Counter".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            hook_provenance: vec![],
            module_consts: Default::default(),
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags =
            SetterInRender.check(&RuleCtx::new(&prog("Counter", &result), result.component));
        assert!(!diags.is_empty(), "setter in render body should warn");
        assert_eq!(diags[0].severity(), Severity::Error);
    }
}
