use crate::rules::RuleCtx;
use std::collections::{HashMap, HashSet};

use crate::domains::StateValue;
use crate::ir::{
    ComponentId,
    cfg::CFG,
    expr::Expr,
    stmt::Stmt,
    types::{BlockId, HookLabel, Var},
};

use crate::engine::guards::{Invariance, LoopFacts, converges_once_written};
use crate::engine::triggers::{Always, Rerun};
use crate::engine::{Change, SlotWriter, WriterPhase, WriterRegion, triggers_of};
use crate::ir::hooks::HookEntry;
use crate::rules::api::query::must_render_write;
use crate::rules::helpers::eval_in_exit_env;
use crate::rules::{
    Diagnostic, ExitDominance, MustResult, Rule, SetterRef, Severity, must_alias_scopes,
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
        group_hand_overs(out, &ctx.program().display_name(ctx.component()))
    }
}

/// One row per hand-over (#377): the rows of one rule that hand setters to
/// code with no timing summary at one site of this component become one row
/// listing them all. The key is (rule, site), within the component the rule
/// runs on: the claim is about one statement of that component handing
/// values to one unseen callee, and the setters those values carry are what
/// it hands, not separate findings — every other row, a setter called
/// during render included, keeps one row per setter. A row with no site is
/// never grouped. The setters are listed in their sorted order, each once;
/// `var`, `hook_label` and the witness are the first one's, its peers'
/// distinct witness steps follow.
fn group_hand_overs(rows: Vec<Diagnostic>, component: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::with_capacity(rows.len());
    let mut groups: Vec<Vec<Diagnostic>> = Vec::new();
    let mut at: HashMap<(std::borrow::Cow<'static, str>, crate::ir::SourceRange), usize> =
        HashMap::new();
    for d in rows {
        let Some(range) = d.range.filter(|_| !d.setters.is_empty()) else {
            out.push(d);
            continue;
        };
        match at.entry((d.rule.clone(), range)) {
            std::collections::hash_map::Entry::Occupied(g) => groups[*g.get()].push(d),
            std::collections::hash_map::Entry::Vacant(g) => {
                g.insert(groups.len());
                groups.push(vec![d]);
            }
        }
    }
    for mut group in groups {
        group.sort_by(|a, b| {
            (&a.setters, a.hook_label, &a.var).cmp(&(&b.setters, b.hook_label, &b.var))
        });
        let mut rest = group.into_iter();
        let Some(mut row) = rest.next() else { continue };
        for d in rest {
            for s in d.setters {
                if !row.setters.contains(&s) {
                    row.setters.push(s);
                }
            }
            for n in d.notes {
                if !row.notes.contains(&n) {
                    row.notes.push(n);
                }
            }
        }
        row.message = hand_over_message(&row.setters, component);
        out.push(row);
    }
    out
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
        // What a settling proof reads as held across the passes of the
        // render loop (#331): only the writes that may run in a render pass
        // run between two of them. Made once, when a proof needs it.
        let facts = std::cell::OnceCell::new();
        let moving = render_pass_slots(result, component);
        let store_moves = comp_result
            .slot_writers
            .iter()
            .any(|w| w.writes_store() && in_render_pass(w));
        let free = |v: &str| v == comp_result.param || comp_result.module_consts.contains_key(v);
        let between = || Between {
            invariance: facts
                .get_or_init(|| LoopFacts::of(comp_result, ctx.cache().written()))
                .invariance(store_moves),
            render: &comp_result.render_cfg,
            moving: &moving,
            free: &free,
        };
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
                let between = between();
                let render = &comp_result.render_cfg;
                let settles = match site {
                    None => converges_once_written(
                        render,
                        bid,
                        &state_vals,
                        &members,
                        w.slot,
                        &eval_in_exit_env(arg, comp_result),
                        Some(arg),
                        &|e| between.holds(e, render, &|x| eval_in_exit_env(x, comp_result)),
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
                        &between,
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
            let mut wording = Wording::of(w, comp_result, names);
            if let Some(name) = shared.then(slot_name).flatten() {
                wording.name = Some(names.name(&name).into_owned());
            }
            let setter = wording.setter();
            let handed = (!sync)
                .then(|| wording.handed(w, w.owner.map(|p| result.display_name(p))))
                .flatten();
            let (rule_name, message) = match (w.owner, &wording.through, &handed) {
                // Handed to code with no timing summary: the row of its site
                // (`group_hand_overs`).
                (_, _, Some(h)) => (
                    hand_over_rule(h),
                    hand_over_message(std::slice::from_ref(h), &result.display_name(component)),
                ),
                // A write in an accessor's body the render's read or write
                // runs (#337).
                (None, _, _) if sync && wording.accessor().is_some() => {
                    ("setter-in-render", sync_accessor_message(&wording))
                }
                (None, None, _) if sync => (
                    "setter-in-render",
                    format!(
                        "{setter} called directly in the render body, \
                         move this call into a useEffect or an event handler"
                    ),
                ),
                (None, Some(through), _) if sync => (
                    "setter-in-render",
                    format!(
                        "{setter} called in the render body through `{through}`, \
                         move this call into a useEffect or an event handler"
                    ),
                ),
                (None, _, _) => ("setter-in-render", unknown_accessor_message(&wording)),
                (Some(parent), through, _) if sync => (
                    "cross-setter-in-render",
                    format!(
                        "{} called during render of `{}`{}, which triggers a parent \
                         re-render on every render",
                        wording.of_parent(&result.display_name(parent)),
                        result.display_name(component),
                        through
                            .as_ref()
                            .map_or_else(String::new, |t| format!(" through `{t}`"))
                    ),
                ),
                (Some(parent), _, _) => {
                    let (accessor, verb) = wording.accessor().unwrap_or_default();
                    (
                        "cross-setter-in-render",
                        format!(
                            "{} is called{} in {accessor}. If the render {verb} it, `{}` \
                             re-renders its parent on every render",
                            wording.of_parent(&result.display_name(parent)),
                            wording.through_link(),
                            result.display_name(component)
                        ),
                    )
                }
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
            if let Some(h) = handed {
                d = d.with_setter(h);
            }
            // Witness (ADR-019): the render-time setter call itself.
            d = d.with_step(
                wording.step(w),
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
                let wording = Wording::of(w, comp_result, names);
                let handed = (!sync && w.owner.is_none())
                    .then(|| wording.handed(w, None))
                    .flatten();
                let (rule_name, message) = match (w.owner, &handed) {
                    (_, Some(h)) => (
                        hand_over_rule(h),
                        hand_over_message(std::slice::from_ref(h), &result.display_name(component)),
                    ),
                    (None, _) if sync => (
                        "setter-in-render",
                        format!(
                            "{} called in a useMemo body{}, which runs during render: \
                             move this call into a useEffect or an event handler",
                            wording.setter(),
                            wording
                                .through
                                .as_ref()
                                .map_or_else(String::new, |t| format!(" through `{t}`"))
                        ),
                    ),
                    (None, _) => ("setter-in-render", unknown_accessor_message(&wording)),
                    (Some(parent), _) => (
                        "cross-setter-in-render",
                        format!(
                            "{} called in a useMemo body{} during render of `{}`, which \
                             triggers a parent re-render",
                            wording.of_parent(&result.display_name(parent)),
                            wording
                                .through
                                .as_ref()
                                .map_or_else(String::new, |t| format!(" through `{t}`")),
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
                if let Some(h) = handed {
                    d = d.with_setter(h);
                }
                d = d.with_step(
                    wording.step(w),
                    None,
                    w.span,
                    &ctx.names(&crate::rules::api::witness::fallback_name),
                );
                Some(d)
            })
            .collect()
    }
}

/// The `Unknown`-phase wording of a write in an accessor's body (#321): it
/// runs wherever its member is read or written, the render included.
fn unknown_accessor_message(wording: &Wording) -> String {
    let (accessor, verb) = wording.accessor().unwrap_or_default();
    format!(
        "{} is called{} in {accessor}. If the render {verb} it, this re-renders on \
         every render",
        wording.setter(),
        wording.through_link()
    )
}

/// The rule a hand-over row of `setter` belongs to: a parent's setter is
/// `cross-setter-in-render`'s.
fn hand_over_rule(setter: &SetterRef) -> &'static str {
    match setter.parent {
        None => "setter-in-render",
        Some(_) => "cross-setter-in-render",
    }
}

/// The wording of a hand-over row (#377). A ⊤ row is not a call in the
/// render body: the setters reached a callee the analyzer has no summary for
/// (`handleSubmit(cb)`, `composeEventHandlers(a, cb)`, a package's `<Button
/// onClick>`), so every phase is possible and the render pass is one of them.
/// Saying "called directly in the render body" of that row would state as
/// fact the one thing the walk could not establish. One setter is named as
/// a write's message names it; several are counted, and the first
/// [`SetterRef::NAMED`] listed (the row's `setters` holds them all). What
/// they went through is said once when they share it. `component` is the
/// component the rule runs on, which a parent's setter re-renders the
/// parent from.
fn hand_over_message(setters: &[SetterRef], component: &str) -> String {
    let through = setters.first().and_then(|s| s.through.as_ref());
    let shared = setters.iter().all(|s| s.through.as_ref() == through);
    let clause = through
        .filter(|_| shared)
        .map_or_else(String::new, |t| format!(" through `{t}`,"));
    let parents = setters.first().is_some_and(|s| s.parent.is_some());
    let (head, it, them, renders) = match setters {
        [s] => (
            match &s.parent {
                None => format!("{} is", setter_phrase(s.name.as_deref())),
                Some(p) => format!(
                    "{} is",
                    parent_setter_phrase(s.name.as_deref(), s.through.is_some(), p)
                ),
            },
            "it",
            "it",
            match parents {
                false => "this re-renders on every render".to_string(),
                true => format!("`{component}` re-renders its parent on every render"),
            },
        ),
        _ => (
            match parents {
                false => format!("{} setters are", setters.len()),
                true => format!("{} state setters of parent components are", setters.len()),
            },
            "one of them",
            "them",
            match parents {
                false => "this re-renders on every render".to_string(),
                true => format!("`{component}` re-renders that setter's parent on every render"),
            },
        ),
    };
    let list = match setters {
        [_] => String::new(),
        _ => {
            let named: Vec<String> = setters
                .iter()
                .take(SetterRef::NAMED)
                .map(|s| s.entry(!shared))
                .collect();
            let list = match (setters.len() - named.len(), named.split_last()) {
                (0, Some((last, init))) => format!("{} and {last}", init.join(", ")),
                (more, _) => format!("{} and {more} more", named.join(", ")),
            };
            format!(" ({list})")
        }
    };
    format!(
        "{head} handed to a callee{clause} with no timing summary{list}. If that callee runs \
         {it} during render, {renders}; if it runs {them} only from event handlers, there is \
         nothing to fix"
    )
}

/// "setter `setN`", or "a state setter" for a slot the source binds to no
/// name.
fn setter_phrase(name: Option<&str>) -> String {
    match name {
        Some(n) => format!("setter `{n}`"),
        None => "a state setter".to_string(),
    }
}

/// A parent's setter: the prop the component holds it by when the prop is
/// that setter (`through` is false), else the setter.
fn parent_setter_phrase(name: Option<&str>, through: bool, parent: &str) -> String {
    match (name, through) {
        (Some(n), false) => format!("prop `{n}` (a state setter of parent `{parent}`)"),
        _ => format!("{} of parent `{parent}`", setter_phrase(name)),
    }
}

/// The wording of a write in an accessor's body that a member read or write
/// of the render runs (#337): the render does what runs it.
fn sync_accessor_message(wording: &Wording) -> String {
    let (accessor, verb) = wording.accessor().unwrap_or_default();
    format!(
        "{} is called{} in {accessor}, and the render {verb} it: move this call into a \
         useEffect or an event handler",
        wording.setter(),
        wording.through_link()
    )
}

/// How a message names the setter a row writes through (D2,
/// [`SlotWriter::named`]): as the source spells it at the call when that is
/// the setter, else the slot's setter and what the write went through — an
/// object handed to a callee, a closure of another component. No message
/// calls a value that is not a setter one.
struct Wording {
    /// The setter's name, printed; `None` for a slot the source binds to no
    /// name.
    name: Option<String>,
    /// The same name, unprinted, for the witness step.
    raw: Option<Var>,
    /// What the write went through, printed, when that is not the setter.
    through: Option<String>,
    /// The accessor whose body the write sits in ([`SlotWriter::accessor`]).
    accessor: Option<crate::engine::setters::AccessorOf>,
}

impl Wording {
    fn of(
        w: &SlotWriter,
        comp: &crate::engine::AnalysisResult<StateValue>,
        names: &crate::ir::SourceNames,
    ) -> Self {
        let (raw, through) = w.named(&comp.calls, comp.component);
        Wording {
            name: raw.as_ref().map(|n| names.name(n).into_owned()),
            raw,
            through: through.map(|t| names.name(t).into_owned()),
            accessor: w.accessor.clone(),
        }
    }

    /// Where a write in an accessor's body runs (#321), and what the render
    /// does that would run it: "the getter of `p`, which runs at every read
    /// of `p`", "reads".
    fn accessor(&self) -> Option<(String, &'static str)> {
        use crate::ir::expr::Accessor;
        let (half, member) = self.accessor.as_ref()?;
        let (kind, act, verb) = match half {
            Accessor::Get => ("getter", "read", "reads"),
            Accessor::Set => ("setter", "write", "writes"),
        };
        let place = match member {
            Some(m) => format!("the {kind} of `{m}`, which runs at every {act} of `{m}`"),
            None => format!("a {kind} of a computed member, which runs at every {act} of it"),
        };
        Some((place, verb))
    }

    /// " through `payload`" when the write went through a value that is not
    /// the setter.
    fn through_link(&self) -> String {
        self.through
            .as_ref()
            .map_or_else(String::new, |t| format!(" through `{t}`"))
    }

    /// "setter `setN`", or "a state setter".
    fn setter(&self) -> String {
        setter_phrase(self.name.as_deref())
    }

    /// A parent's setter: the prop this component calls when the prop is
    /// that setter, else the setter.
    fn of_parent(&self, parent: &str) -> String {
        parent_setter_phrase(self.name.as_deref(), self.through.is_some(), parent)
    }

    /// The setter a hand-over row of `w` stands for (#377), `parent` the
    /// display name of the component whose slot it writes when that is not
    /// this one. `None` for a write in an accessor's body, which runs at a
    /// member's read or write and is no hand-over.
    fn handed(&self, w: &SlotWriter, parent: Option<String>) -> Option<SetterRef> {
        self.accessor.is_none().then(|| SetterRef {
            name: self.name.clone(),
            var: parent.as_ref().map(|_| w.setter.clone()),
            parent,
            through: self.through.clone(),
        })
    }

    /// The witness step of the row's call: a call of the setter, or, for a
    /// slot no name binds, a call of the value it went through.
    fn step(&self, w: &SlotWriter) -> crate::rules::Step {
        match &self.raw {
            Some(raw) => crate::rules::Step::Call {
                callee: raw.clone(),
                class: crate::rules::EffectClass::Setter,
            },
            None => crate::rules::Step::Call {
                callee: w.setter.clone(),
                class: crate::rules::EffectClass::Unknown,
            },
        }
    }
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
    between: &Between<'_>,
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
    let value = |e: &Expr, heap: &mut crate::domains::stores::Heap| {
        crate::engine::eval::eval_in_stores(
            e,
            &env,
            comp.component,
            &comp.state_store,
            &comp.memo_store,
            heap,
        )
    };
    let mut heap = comp.heap.clone();
    let written = value(arg, &mut heap);
    converges_once_written(
        body,
        block,
        &slots,
        members,
        label,
        &written,
        Some(arg),
        &|e| between.holds(e, body, &|x| value(x, &mut comp.heap.clone())),
        &env,
        &mut |e| value(e, &mut heap),
    )
}

/// What a settling proof reads as held across the passes of the render loop
/// ([`Invariance::holds_between_passes`], #331).
struct Between<'a> {
    invariance: Invariance<'a>,
    render: &'a CFG,
    /// The slots a write that may run in a render pass writes.
    moving: &'a HashSet<HookLabel>,
    /// A name nothing in the component binds that holds: its props, a module
    /// constant.
    free: &'a dyn Fn(&str) -> bool,
}

impl Between<'_> {
    /// Does `e`, read in `body`, denote one value on every pass? `value`
    /// evaluates it where it is read.
    fn holds(&self, e: &Expr, body: &CFG, value: &dyn Fn(&Expr) -> StateValue) -> bool {
        self.invariance
            .holds_between_passes(e, body, self.render, self.free, self.moving, value)
    }
}

/// A write that may run in a render pass of the component whose slot it
/// writes: of a render or memo body, synchronously or at an unknown time.
fn in_render_pass(w: &SlotWriter) -> bool {
    matches!(w.region, WriterRegion::Render | WriterRegion::Memo(_))
        && matches!(
            w.phase,
            WriterPhase::Render | WriterPhase::Memo | WriterPhase::Unknown
        )
}

/// The slots of `component` some write that may run in a render pass writes:
/// its own, or another component's through a prop.
fn render_pass_slots(
    result: &crate::engine::ProgramAnalysisResult,
    component: crate::ir::ComponentId,
) -> HashSet<HookLabel> {
    result
        .components
        .iter()
        .flat_map(|(c, r)| {
            r.slot_writers.iter().filter(move |w| {
                in_render_pass(w) && w.owner.unwrap_or(*c) == component && !w.writes_store()
            })
        })
        .map(|w| w.slot)
        .collect()
}

/// Slot `label` of `component` has exactly one write that may run in a
/// render pass — of its own render or memo bodies, or of another component's
/// (a prop) — in the writer relation.
fn sole_render_writer(
    result: &crate::engine::ProgramAnalysisResult,
    component: crate::ir::ComponentId,
    label: HookLabel,
) -> bool {
    let render_pass = |w: &&SlotWriter| w.slot == label && in_render_pass(w);
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

    fn handed(name: Option<&str>, parent: Option<&str>, through: Option<&str>) -> SetterRef {
        SetterRef {
            name: name.map(str::to_string),
            parent: parent.map(str::to_string),
            through: through.map(str::to_string),
            var: parent.map(|_| "onX".to_string()),
        }
    }

    #[test]
    fn a_hand_over_names_one_setter_as_a_write_does_and_counts_several() {
        // #377: one setter reads as the row of its write.
        assert_eq!(
            hand_over_message(&[handed(Some("setN"), Some("P"), None)], "C"),
            "prop `setN` (a state setter of parent `P`) is handed to a callee with no timing \
             summary. If that callee runs it during render, `C` re-renders its parent on every \
             render; if it runs it only from event handlers, there is nothing to fix"
        );
        // What they went through, said once when they share it.
        let shared = [
            handed(Some("setA"), None, Some("payload")),
            handed(None, None, Some("payload")),
        ];
        assert!(
            hand_over_message(&shared, "C").starts_with(
                "2 setters are handed to a callee through `payload`, with no timing summary \
                 (`setA` and a state setter). If that callee runs one of them"
            ),
            "{}",
            hand_over_message(&shared, "C")
        );
        // Else on each, and past three, counted.
        let mixed = [
            handed(Some("setA"), Some("P"), Some("o")),
            handed(Some("setB"), Some("P"), None),
            handed(Some("setC"), Some("Q"), None),
            handed(Some("setD"), Some("Q"), None),
            handed(Some("setE"), Some("Q"), None),
        ];
        assert!(
            hand_over_message(&mixed, "C").starts_with(
                "5 state setters of parent components are handed to a callee with no timing \
                 summary (`setA` of `P` through `o`, `setB` of `P`, `setC` of `Q` and 2 more). \
                 If that callee runs one of them during render, `C` re-renders that setter's \
                 parent on every render"
            ),
            "{}",
            hand_over_message(&mixed, "C")
        );
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
            ref_passing: Default::default(),
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags =
            SetterInRender.check(&RuleCtx::new(&prog("Counter", &result), result.component));
        assert!(!diags.is_empty(), "setter in render body should warn");
        assert_eq!(diags[0].severity(), Severity::Error);
    }
}
