use crate::rules::RuleCtx;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::impls::Stability,
    ir::{
        SourceRange,
        cfg::CFG,
        expr::Expr,
        free_vars::{AccessPath, compute_free_paths, dep_paths, path_covered},
        hooks::{Arity, HookEntry},
        types::{HookLabel, Var},
    },
};

use crate::engine::registrations::Firing;
use crate::ir::ComponentId;
use crate::rules::helpers::churn::eval_in_exit_env;
use crate::rules::{
    Certified, Diagnostic, EffectClass, MustResult, OnAllPaths, Rule, Severity, Step, ValueClass,
    all_setter_labels, collect_fn_bindings, collect_setter_calls_with_extra, fn_lit_binding,
    may_written_slots, memo_val_labels, must_on_all_paths, resolve_setter_aliases, state_slot_name,
    state_val_labels,
};

/// Fires when a callback that **outlives the render** — handed to
/// `setInterval`, `addEventListener`, `subscribe`, `setTimeout`, a promise
/// `.then`… inside a `useEffect` — captures a state value the effect's deps
/// array does not cover. The callback keeps the value from the render that
/// last ran the effect; the state moves on without it.
///
/// Distinct from `missing-deps` (eslint-parity: *any* uncovered capture):
/// this rule proves the *consequence*. The version domain (ADR-017) says the
/// slot changes only at setter events; the deps array says the effect never
/// re-runs at those events; the registration says the stale copy keeps
/// executing. When the callback also **writes** the slot it reads
/// (`setN(n + 1)` in an interval), the freeze is self-inflicted and certain:
/// the state can never advance past its first update — Error.
///
/// Stratification (three-level doctrine — Error only on a triple must):
/// - **Error**: repeating registrar ∧ deps `[]` (never re-runs) ∧ the
///   registration is on all paths of the effect body ∧ the callback writes a
///   slot it captures.
/// - **Warning**: everything else that survives the kills below — one-shot
///   registrars (`setTimeout`, `.then`: bounded staleness window), non-empty
///   deps (freeze lasts until an unrelated dep changes), conditional or
///   nested registration, foreign/unknown slots.
///
/// Stays silent when (each kill is a proof, not a heuristic):
/// - the captured path is covered by the deps array (the effect re-runs and
///   re-registers when the value changes) — includes the *identity* form:
///   the registered callback variable itself is a dep (`[tick]` where `tick`
///   is re-created when its captures change);
/// - the capture reads `ref.current` or any `Stable` value (the canonical
///   ref-mirror fix);
/// - the callback uses the functional updater (`setN(n => n + 1)`): the
///   updater's parameter shadows the slot, nothing stale is read;
/// - the effect has no deps array (re-runs every render → fresh capture);
/// - the slot's setter is never referenced anywhere in the component: the
///   slot provably never changes, so the capture can never go stale.
pub struct StaleClosure;

/// Resolve the registered callback expression to `(params, body)`.
/// `None` = opaque (imported fn, conditional re-binding) — skipped, matching
/// the certainty bar of `missing-deps`' `fn_lit_binding`.
///
/// A callback **variable that is itself a dep** resolves to `None` on
/// purpose: the effect re-runs whenever the function's identity changes, and
/// a render-created closure (or a `useCallback` keyed on its captures)
/// changes identity exactly when its captures do — the re-registration
/// carries a fresh capture.
fn resolve_callback<'a>(
    cb: &'a Expr,
    declared: &[AccessPath],
    effect_body: &'a CFG,
    render_cfg: &'a CFG,
    memo_vars: &HashMap<Var, HookLabel>,
    callback_hooks: &HashMap<HookLabel, (&'a [Var], &'a CFG)>,
) -> Option<(Option<&'a str>, &'a [Var], &'a CFG)> {
    match cb.peel_ts() {
        Expr::FnLit {
            params, body_cfg, ..
        } => Some((None, params, body_cfg)),
        Expr::Var(v) => {
            let as_path = AccessPath {
                root: v.clone(),
                segments: vec![],
            };
            if path_covered(&as_path, declared) {
                return None; // identity-covered: re-registered on change
            }
            if let Some((params, body)) = fn_lit_binding(v, effect_body) {
                return Some((Some(v.as_str()), params, body));
            }
            if let Some((params, body)) = fn_lit_binding(v, render_cfg) {
                return Some((Some(v.as_str()), params, body));
            }
            if let Some(label) = memo_vars.get(v.as_str())
                && let Some((params, body)) = callback_hooks.get(label)
            {
                return Some((Some(v.as_str()), params, body));
            }
            None
        }
        Expr::CallbackVal(label) => callback_hooks
            .get(label)
            .map(|(params, body)| (None, *params, *body)),
        _ => None,
    }
}

/// Slots a captured path's root resolves to.
struct RootSlots {
    /// This component's slots (nameable, self-write provable).
    local: Vec<HookLabel>,
    /// Versioned by another component's slot, or by unknown slots
    /// (`VersionedTop`) — real staleness, but nothing local to prove
    /// against: Warning ceiling, no never-written kill.
    foreign: bool,
}

fn resolve_root_slots(
    root: &Var,
    state_vals: &HashMap<Var, HookLabel>,
    component: ComponentId,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> RootSlots {
    // Syntactic first: state bindings and their aliases. This is what makes
    // primitive slots work — a counter's product value has a ⊥ reference
    // slot, so the read-side `Versioned` conversion (ADR-017) never applies
    // to it; the binding chain is the proof instead.
    if let Some(&l) = state_vals.get(root) {
        return RootSlots {
            local: vec![l],
            foreign: false,
        };
    }
    // Domain fallback: memo chains, destructured props, object state
    // aliases — anything whose reference slot carries version labels.
    let env_exit = comp_result.exit_env();
    if !env_exit.contains(root) {
        return RootSlots {
            local: vec![],
            foreign: false,
        };
    }
    let val = eval_in_exit_env(&Expr::Var(root.clone()), comp_result);
    match &val.reference {
        Stability::Versioned(labels) => {
            let mut local: Vec<HookLabel> = labels
                .iter()
                .filter(|(c, _)| *c == component)
                .map(|(_, l)| *l)
                .collect();
            local.sort_unstable();
            let foreign = labels.iter().any(|(c, _)| *c != component);
            RootSlots { local, foreign }
        }
        Stability::VersionedTop => RootSlots {
            local: vec![],
            foreign: true,
        },
        _ => RootSlots {
            local: vec![],
            foreign: false,
        },
    }
}

/// Best finding for one captured path within one effect.
struct PathFinding {
    severity: Severity,
    /// The must-reach proof backing the Error tier (`None` for Warning). Carried
    /// through the per-path dedup so `Diagnostic::error` can mint from it.
    proof: Option<Certified<OnAllPaths>>,
    registrar: String,
    reg_span: Option<SourceRange>,
    resolved_via: Option<String>,
    slots: Vec<HookLabel>,
    /// Span of the callback's own write to a captured slot, when proven.
    self_write: Option<(HookLabel, Option<SourceRange>)>,
    mount_only: bool,
}

impl StaleClosure {
    const NAME: &'static str = "stale-closure";
}

impl Rule for StaleClosure {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        // Applicable when some deps-gated effect registers a long-lived
        // callback at all.
        let comp_result = result.components.get(&component)?;
        let applicable = comp_result.hooks.iter().any(|h| {
            let HookEntry::Effect { label, deps, .. } = h else {
                return false;
            };
            deps.is_declared() && comp_result.registrations.iter().any(|r| r.effect == *label)
        });
        applicable.then_some(crate::rules::SafeCheck {
            rule: Self::NAME,
            message: "no long-lived callback captures a stale state value",
        })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let comp_result = &result.components[&component];
        let render_cfg = &comp_result.render_cfg;

        let state_vals_render = resolve_setter_aliases(render_cfg, &state_val_labels(render_cfg));
        let memo_vars = resolve_setter_aliases(render_cfg, &memo_val_labels(render_cfg));

        let setter_labels = all_setter_labels(comp_result);
        let written = may_written_slots(render_cfg, &comp_result.hooks, &setter_labels);
        let render_fns = collect_fn_bindings(render_cfg);
        let callback_hooks: HashMap<HookLabel, (&[Var], &CFG)> = comp_result
            .hooks
            .iter()
            .filter_map(|h| match h {
                HookEntry::Callback {
                    label,
                    body_cfg,
                    params,
                    ..
                } => Some((*label, (params.as_slice(), body_cfg))),
                _ => None,
            })
            .collect();
        let name_of = |l: HookLabel| state_slot_name(l, &state_vals_render);

        let mut diags = Vec::new();

        for hook in &comp_result.hooks {
            let HookEntry::Effect {
                label: eff_label,
                body_cfg,
                deps,
                span: eff_span,
            } = hook
            else {
                continue;
            };
            // No deps array: the effect re-runs every render, every
            // registration gets a fresh capture (the *old* one leaking is a
            // cleanup problem, not a staleness one). A deps argument the
            // engine cannot read is NOT that case — the effect is gated, so it
            // is checked with nothing covered.
            if !deps.is_declared() {
                continue;
            }
            // The coverage view, not every visible element: a flattened
            // spread covers its contents, never its own identity.
            let declared = deps
                .list()
                .map_or_else(Vec::new, |l| dep_paths(&l.covering()));
            let mount_only = matches!(deps.list(), Some(l) if l.arity == Arity::Exact(0));

            let mut fn_bodies = collect_fn_bindings(body_cfg);
            for (k, v) in &render_fns {
                fn_bodies.entry(k.clone()).or_insert_with(|| Arc::clone(v));
            }
            // The engine's registration relation (ADR-034), not a second scan.
            let regs: Vec<_> = comp_result
                .registrations
                .iter()
                .filter(|r| r.effect == *eff_label)
                .collect();
            if regs.is_empty() {
                continue;
            }

            // Effect-local aliases (`const cur = n;` inside the body) extend
            // the render map so captures of them still root at the slot.
            let state_vals = resolve_setter_aliases(body_cfg, &state_vals_render);

            // Best finding per captured path (BTreeMap: deterministic order).
            let mut best: BTreeMap<String, PathFinding> = BTreeMap::new();

            for reg in &regs {
                let Some((via, params, cb_body)) = resolve_callback(
                    &reg.callback,
                    &declared,
                    body_cfg,
                    render_cfg,
                    &memo_vars,
                    &callback_hooks,
                ) else {
                    continue;
                };
                let mut caps: Vec<AccessPath> = compute_free_paths(cb_body)
                    .into_iter()
                    .filter(|p| !params.contains(&p.root))
                    .collect();
                caps.sort_by_key(|p| p.to_string());

                for path in caps {
                    if path_covered(&path, &declared) {
                        continue;
                    }
                    let roots = resolve_root_slots(&path.root, &state_vals, component, comp_result);
                    if roots.local.is_empty() && !roots.foreign {
                        continue;
                    }
                    // Never-written kill: all resolved slots are local and
                    // none can ever be written → the capture never goes stale.
                    let live_local: Vec<HookLabel> = roots
                        .local
                        .iter()
                        .copied()
                        .filter(|l| written.contains(l))
                        .collect();
                    if live_local.is_empty() && !roots.foreign {
                        continue;
                    }

                    // Does the callback itself write a slot it captures?
                    let slot_setters: HashSet<Var> = setter_labels
                        .iter()
                        .filter(|(_, l)| live_local.contains(l))
                        .map(|(v, _)| v.clone())
                        .collect();
                    let self_write = if slot_setters.is_empty() {
                        None
                    } else {
                        let mut calls =
                            collect_setter_calls_with_extra(cb_body, &slot_setters, 2, &fn_bodies);
                        calls.sort_by_key(|c| c.span.map_or((u32::MAX, u32::MAX), |r| r.pos_key()));
                        calls
                            .first()
                            .and_then(|c| setter_labels.get(&c.var).map(|l| (*l, c.span)))
                    };

                    // must-reach as a certified proof (the only path to Error).
                    let reach_proof = reg.block_id.and_then(|b| {
                        match must_on_all_paths(body_cfg, &HashSet::from([b])) {
                            MustResult::All(c) => Some(c),
                            _ => None,
                        }
                    });
                    let is_error = reg.firing == Firing::Repeating
                        && mount_only
                        && reach_proof.is_some()
                        && self_write.is_some();
                    let severity = if is_error {
                        Severity::Error
                    } else {
                        Severity::Warning
                    };
                    let proof = if is_error { reach_proof } else { None };

                    let rank = |s: Severity| match s {
                        Severity::Error => 2,
                        Severity::Warning => 1,
                        Severity::Info => 0,
                    };
                    let pos =
                        |s: Option<SourceRange>| s.map_or((u32::MAX, u32::MAX), |r| r.pos_key());
                    let candidate = PathFinding {
                        severity,
                        proof,
                        registrar: reg.display.clone(),
                        reg_span: reg.span,
                        resolved_via: via.map(str::to_string),
                        slots: live_local.clone(),
                        self_write,
                        mount_only,
                    };
                    match best.entry(path.to_string()) {
                        std::collections::btree_map::Entry::Vacant(e) => {
                            e.insert(candidate);
                        }
                        std::collections::btree_map::Entry::Occupied(mut e) => {
                            let cur = e.get();
                            if rank(severity) > rank(cur.severity)
                                || (rank(severity) == rank(cur.severity)
                                    && pos(reg.span) < pos(cur.reg_span))
                            {
                                e.insert(candidate);
                            }
                        }
                    }
                }
            }

            for (path, f) in best {
                let message = match (f.severity, f.mount_only) {
                    (Severity::Error, _) => format!(
                        "the `{reg}` callback registered by this mount-only effect reads \
                         `{path}` and writes it back. `{path}` was captured once at mount, \
                         so every firing recomputes from the same frozen value and the \
                         state can never advance past its first update",
                        reg = f.registrar,
                    ),
                    (_, true) => format!(
                        "`{path}` is captured by the `{reg}` callback registered in this \
                         mount-only effect, so the callback outlives the render and keeps \
                         reading the mount-time value after `{path}` changes",
                        reg = f.registrar,
                    ),
                    (_, false) => format!(
                        "`{path}` is captured by the `{reg}` callback registered in this \
                         effect, but the deps array does not cover it, so after `{path}` \
                         changes, the callback keeps reading the value from the effect's \
                         last run",
                        reg = f.registrar,
                    ),
                };
                let mut d = match (f.severity, f.proof) {
                    (Severity::Error, Some(proof)) => {
                        Diagnostic::error("stale-closure", proof, message)
                    }
                    _ => Diagnostic::warn("stale-closure", message),
                }
                .with_label(*eff_label)
                .with_var(path.clone());
                if let Some(r) = f.reg_span.or(*eff_span) {
                    d = d.with_range(r);
                }
                // Witness (ADR-019): the registration, the resolution of a
                // named callback, the capture, and the self-write when proven.
                d = d.with_step(
                    Step::Call {
                        callee: f.registrar.clone(),
                        class: EffectClass::Effectful,
                    },
                    Some(*eff_label),
                    f.reg_span,
                    &name_of,
                );
                if let Some(via) = &f.resolved_via {
                    d = d.with_step(
                        Step::Resolve {
                            name: via.clone(),
                            target: crate::rules::ResolveTarget::LocalFn,
                        },
                        None,
                        None,
                        &name_of,
                    );
                }
                d = d.with_step(
                    Step::Capture { what: path.clone() },
                    f.slots.first().copied(),
                    f.reg_span,
                    &name_of,
                );
                if let Some((slot, span)) = f.self_write {
                    d = d.with_step(
                        Step::Write {
                            slot,
                            value: ValueClass::Unknown,
                        },
                        Some(slot),
                        span,
                        &name_of,
                    );
                }
                diags.push(d);
            }
        }

        diags
    }
}
