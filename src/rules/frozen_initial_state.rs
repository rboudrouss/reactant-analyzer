use std::collections::{HashMap, HashSet};

use crate::{
    domains::{StateValue, impls::Stability},
    engine::{AnalysisResult, ProgramAnalysisResult},
    ir::{
        SourceRange,
        cfg::CFG,
        expr::Expr,
        free_vars::{AccessPath, collect_used_paths, dep_paths, path_covered},
        hooks::HookEntry,
        types::{HookLabel, Symbol, Var},
    },
};

use super::{
    Diagnostic, Rule, Severity, Step, ValueClass, all_setter_labels, collect_fn_bindings,
    collect_setter_calls, collect_setter_calls_with_extra, local_bindings,
    stale_closure::may_written_slots, state_slot_name, state_val_labels,
};

/// Fires when a `useState` initializer reads a prop and nothing ever syncs
/// the slot afterwards: the initializer runs on the **first render only**, so
/// the state freezes at the first prop value while the prop moves on — the
/// classic "my input doesn't update when props change".
///
/// Distinct from `derived-state`: there an effect *does* mirror the source
/// (the fix is to delete the state); here there is no sync at all (the fix is
/// to pick an ownership model — controlled, key-remount, or a deliberate
/// syncing effect).
///
/// Stratification (three-level doctrine — Error only on a full proof chain):
/// - **Error**: the prop is *proven* fed by another component's state slot
///   (`Versioned` reference through ADR-012 top-down analysis), that slot may
///   actually be written in its owner, no effect keyed on the prop (and no
///   render-time write) syncs the local slot, and the local setter never
///   escapes this component (nothing else could sync it).
/// - **Warning**: the freeze is real but the prop's motion is not proven
///   (props ⊤ in intra-only analysis, `VersionedTop`, per-render or unknown
///   values), or the proof chain has a hole (setter escapes, feeding slot's
///   owner unavailable).
/// - **Info**: intent is declared — every seeding prop is *named* for
///   seed-once (`initial*` / `default*`) with unproven motion, or the local
///   slot is never written at all (`const [{ snap }] = useState(...)`: a
///   deliberate mount-time snapshot) — surfaced as advice only.
///
/// Stays silent when (each kill is a proof, not a heuristic):
/// - the initializer reads no prop;
/// - the prop provably never changes: `Stable` value, or every feeding state
///   slot is never written in its owning component (setter unreferenced —
///   same proof as `stale-closure`'s never-written kill, run on the parent);
/// - an effect whose deps cover the prop path (or with no deps array) may
///   write the slot — a sync path exists (its quality is `derived-state`'s
///   business, not ours);
/// - the slot's setter is called during render — the documented
///   adjust-state-during-render pattern (`setter-in-render` owns misuse).
pub struct FrozenInitialState;

/// `props.a.b` / `value` as a plain member chain, `None` for anything else.
fn as_member_chain(e: &Expr) -> Option<AccessPath> {
    match e.peel_ts() {
        Expr::Var(v) => Some(AccessPath {
            root: v.clone(),
            segments: vec![],
        }),
        Expr::FieldAccess { obj, field } => {
            let mut p = as_member_chain(obj)?;
            p.segments.push(field.clone());
            Some(p)
        }
        _ => None,
    }
}

/// Rewrite `path` into props-param-rooted form(s) by chasing single-binding
/// local chains: destructuring preambles (`let value = __p0.value`), aliases,
/// and derived bindings (`const v = props.a ?? d` roots at `props.a`). A
/// multi-write binding is uncertain and not chased. For a complex RHS the
/// outer segments are dropped (wider path — safe for rootedness, and coverage
/// against a wider path only errs toward keeping the finding).
fn normalize_to_prop(
    path: &AccessPath,
    param: &Var,
    bindings: &HashMap<&str, Vec<&Expr>>,
    seen: &mut HashSet<Var>,
) -> Vec<AccessPath> {
    if path.root == *param {
        return vec![path.clone()];
    }
    if !seen.insert(path.root.clone()) {
        return vec![];
    }
    let Some(rhss) = bindings.get(path.root.as_str()) else {
        return vec![];
    };
    let [single] = rhss.as_slice() else {
        return vec![];
    };
    if let Some(base) = as_member_chain(single) {
        let combined = AccessPath {
            root: base.root,
            segments: base
                .segments
                .into_iter()
                .chain(path.segments.iter().cloned())
                .collect(),
        };
        return normalize_to_prop(&combined, param, bindings, seen);
    }
    let mut sub = HashSet::new();
    collect_used_paths(single, &mut sub);
    let mut out = Vec::new();
    for p in sub {
        out.extend(normalize_to_prop(&p, param, bindings, seen));
    }
    out
}

/// One prop read by a `useState` initializer.
struct SeedPath {
    /// Path as written in source (display + env evaluation).
    orig: AccessPath,
    /// Props-param-rooted forms (deps-coverage matching).
    normalized: Vec<AccessPath>,
}

/// `initialValue` / `defaultTab` — the prop's own name declares seed-once
/// intent (uncontrolled-with-default idiom).
fn is_seed_named(p: &AccessPath) -> bool {
    let name = p.segments.last().map(String::as_str).unwrap_or(&p.root);
    let lower = name.to_ascii_lowercase();
    lower.starts_with("initial") || lower.starts_with("default")
}

/// What the domain proves about a seeding prop's motion across renders.
enum Motion {
    /// Provably never changes — kill.
    Still,
    /// Fed by a state slot that may actually be written in its owner.
    Proven {
        slot: HookLabel,
        /// Owner's source-level name for the slot, pre-qualified for display
        /// ("`text` of `Parent`").
        display: String,
        write_span: Option<SourceRange>,
    },
    /// May change, unproven (⊤ props, per-render values, unverifiable owner).
    Unproven,
}

/// Can `slot` ever be written in its owning component, and if so, where is
/// the first provable write site? `(false, _)` is a proof of stillness;
/// `(true, None)` means "referenced somewhere" without a direct call site
/// (setter passed onward).
fn slot_write_evidence(
    owner: &AnalysisResult<StateValue>,
    slot: HookLabel,
) -> (bool, Option<SourceRange>) {
    let setter_labels = all_setter_labels(owner);
    let may = may_written_slots(&owner.render_cfg, &owner.hooks, &setter_labels);
    if !may.contains(&slot) {
        return (false, None);
    }
    let setters: HashSet<Var> = setter_labels
        .iter()
        .filter(|(_, l)| **l == slot)
        .map(|(v, _)| v.clone())
        .collect();
    let render_fns = collect_fn_bindings(&owner.render_cfg);
    let mut spans: Vec<SourceRange> = std::iter::once(&owner.render_cfg)
        .chain(owner.hooks.iter().filter_map(|h| h.body_cfg()))
        .flat_map(|cfg| collect_setter_calls_with_extra(cfg, &setters, 2, &render_fns))
        .filter_map(|c| c.span)
        .collect();
    spans.sort_by_key(|r| r.pos_key());
    (true, spans.first().copied())
}

/// Classify the motion of a seeding prop from its abstract value.
fn classify_motion(val: &StateValue, result: &ProgramAnalysisResult) -> Motion {
    // Version labels live on the reference slot only (`to_stability` erases
    // them when another kind slot is ⊤) — check it first, like
    // `recompute_memo` does.
    if let Stability::Versioned(labels) = &val.reference {
        let mut unverifiable = false;
        for (owner, slot) in labels {
            let Some(owner_result) = result.components.get(owner) else {
                unverifiable = true;
                continue;
            };
            let (writable, write_span) = slot_write_evidence(owner_result, *slot);
            if writable {
                let owner_states = state_val_labels(&owner_result.render_cfg);
                let display = format!(
                    "state {} of `{owner}`",
                    state_slot_name(*slot, &owner_states)
                );
                return Motion::Proven {
                    slot: *slot,
                    display,
                    write_span,
                };
            }
        }
        return if unverifiable {
            Motion::Unproven
        } else {
            // Every feeding slot is owned by an analyzed component and its
            // setter is never referenced there: the prop provably never
            // changes (React state moves only through its setter).
            Motion::Still
        };
    }
    if val.reference == Stability::VersionedTop {
        return Motion::Unproven;
    }
    match val.to_stability() {
        Stability::Bottom | Stability::Stable => Motion::Still,
        _ => Motion::Unproven,
    }
}

/// `true` when some declared dep covers `seed` — matched on the syntactic
/// path AND on the props-rooted normal forms of both sides, so `[value]`,
/// `[props.value]` and `[props]` all cover a seed read as `value`.
fn deps_cover_seed(
    deps: &[Expr],
    seed: &SeedPath,
    param: &Var,
    bindings: &HashMap<&str, Vec<&Expr>>,
) -> bool {
    let declared = dep_paths(deps);
    let mut declared_norm: Vec<AccessPath> = Vec::new();
    for d in &declared {
        declared_norm.extend(normalize_to_prop(d, param, bindings, &mut HashSet::new()));
    }
    if path_covered(&seed.orig, &declared) {
        return true;
    }
    seed.normalized
        .iter()
        .any(|n| path_covered(n, &declared_norm))
}

/// `true` when an alias of the slot's setter is used anywhere outside a
/// direct call or a pure alias binding — passed as a prop, stored in an
/// object, handed to an opaque call. An escaped setter means something we
/// cannot see may sync the slot, so the no-sync claim loses certainty.
fn setter_escapes(comp: &AnalysisResult<StateValue>, aliases: &HashSet<Var>) -> bool {
    fn in_expr(e: &Expr, aliases: &HashSet<Var>) -> bool {
        match e {
            Expr::Var(v) => aliases.contains(v),
            Expr::Call { fn_, args } => {
                let callee_is_alias = matches!(fn_.peel_ts(), Expr::Var(v) if aliases.contains(v));
                (!callee_is_alias && in_expr(fn_, aliases))
                    || args.iter().any(|a| in_expr(a, aliases))
            }
            Expr::FnLit {
                params, body_cfg, ..
            } => {
                // Params shadow same-named outer bindings inside the body.
                let inner: HashSet<Var> = aliases
                    .iter()
                    .filter(|a| !params.contains(a))
                    .cloned()
                    .collect();
                !inner.is_empty() && in_cfg(body_cfg, &inner)
            }
            other => {
                let mut found = false;
                other.for_each_child(&mut |c| found = found || in_expr(c, aliases));
                found
            }
        }
    }
    fn in_cfg(cfg: &CFG, aliases: &HashSet<Var>) -> bool {
        use crate::ir::{cfg::Terminator, stmt::Stmt};
        cfg.blocks.values().any(|block| {
            block.stmts.iter().any(|stmt| match stmt {
                // `let s2 = s1` where both sides are known aliases is the
                // alias chain itself, not an escape.
                Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => match rhs.peel_ts() {
                    Expr::Var(v) if aliases.contains(v) => !aliases.contains(var),
                    _ => in_expr(rhs, aliases),
                },
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    in_expr(obj, aliases)
                        || matches!(key, crate::ir::stmt::MemberKey::Index(i) if in_expr(i, aliases))
                        || in_expr(rhs, aliases)
                }
                Stmt::ExprStmt(e, _) => in_expr(e, aliases),
            }) || match &block.term {
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } => in_expr(e, aliases),
                _ => false,
            }
        })
    }
    let cfgs =
        std::iter::once(&comp.render_cfg).chain(comp.hooks.iter().filter_map(|h| h.body_cfg()));
    for cfg in cfgs {
        if in_cfg(cfg, aliases) {
            return true;
        }
    }
    // Custom-hook args and state/ref initializers can smuggle the setter too.
    comp.hooks.iter().any(|h| match h {
        HookEntry::Custom { args, .. } => args.iter().any(|a| in_expr(a, aliases)),
        HookEntry::State { init, .. } | HookEntry::Ref { init, .. } => {
            !matches!(init.peel_ts(), Expr::StateSetter(_)) && in_expr(init, aliases)
        }
        _ => false,
    })
}

/// Prop paths read by a `useState` initializer, `None`-equivalent (empty)
/// when the initializer is prop-free.
fn seed_paths(init: &Expr, param: &Var, bindings: &HashMap<&str, Vec<&Expr>>) -> Vec<SeedPath> {
    let mut used = HashSet::new();
    collect_used_paths(init, &mut used);
    let mut seeds: Vec<SeedPath> = used
        .into_iter()
        .filter_map(|orig| {
            let normalized = normalize_to_prop(&orig, param, bindings, &mut HashSet::new());
            (!normalized.is_empty()).then_some(SeedPath { orig, normalized })
        })
        .collect();
    seeds.sort_by_key(|s| s.orig.to_string());
    seeds
}

impl Rule for FrozenInitialState {
    fn name(&self) -> &'static str {
        "frozen-initial-state"
    }

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        let comp = result.components.get(component)?;
        let bindings = local_bindings(&comp.render_cfg);
        let applicable = comp.hooks.iter().any(|h| {
            matches!(h, HookEntry::State { init, .. }
                if !seed_paths(init, &comp.param, &bindings).is_empty())
        });
        applicable.then_some(super::SafeCheck {
            rule: self.name(),
            message: "no state slot freezes a changing prop's first value",
        })
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let comp = &result.components[component];
        let render_cfg = &comp.render_cfg;
        let bindings = local_bindings(render_cfg);
        let setter_labels = all_setter_labels(comp);
        let state_labels = state_val_labels(render_cfg);
        let render_fns = collect_fn_bindings(render_cfg);
        let locally_written = may_written_slots(render_cfg, &comp.hooks, &setter_labels);
        let name_of = |l: HookLabel| state_slot_name(l, &state_labels);

        let mut diags = Vec::new();

        for hook in &comp.hooks {
            let HookEntry::State { label, init, span } = hook else {
                continue;
            };
            let seeds = seed_paths(init, &comp.param, &bindings);
            if seeds.is_empty() {
                continue;
            }

            let slot_setters: HashSet<Var> = setter_labels
                .iter()
                .filter(|(_, l)| **l == *label)
                .map(|(v, _)| v.clone())
                .collect();

            // Render-time write: the adjust-state-during-render pattern —
            // a sync path exists (misuse belongs to `setter-in-render`).
            if !collect_setter_calls(render_cfg, &slot_setters, 2).is_empty() {
                continue;
            }

            // Effect sync: an effect that may write the slot AND re-runs when
            // the prop changes (deps cover the seed path, or no deps array).
            let synced = comp.hooks.iter().any(|h| {
                let HookEntry::Effect { body_cfg, deps, .. } = h else {
                    return false;
                };
                if collect_setter_calls_with_extra(body_cfg, &slot_setters, 2, &render_fns)
                    .is_empty()
                {
                    return false;
                }
                match deps {
                    None => true, // re-runs every render
                    Some(deps) => seeds
                        .iter()
                        .any(|s| deps_cover_seed(deps, s, &comp.param, &bindings)),
                }
            });
            if synced {
                continue;
            }

            // Classify each seeding prop; keep the strongest surviving verdict.
            // proven = (seed path, owner slot, qualified display, write span).
            let mut proven: Option<(String, HookLabel, String, Option<SourceRange>)> = None;
            let mut unproven_path: Option<String> = None;
            let mut all_seed_named = true;
            for seed in &seeds {
                let mut expr = Expr::Var(seed.orig.root.clone());
                for seg in &seed.orig.segments {
                    expr = Expr::FieldAccess {
                        obj: Box::new(expr),
                        field: seg.clone(),
                    };
                }
                let val = eval_with_heap(&expr, comp);
                match classify_motion(&val, result) {
                    Motion::Still => continue,
                    Motion::Proven {
                        slot,
                        display,
                        write_span,
                    } => {
                        all_seed_named &= is_seed_named(&seed.orig);
                        if proven.is_none() {
                            proven = Some((seed.orig.to_string(), slot, display, write_span));
                        }
                    }
                    Motion::Unproven => {
                        all_seed_named &= is_seed_named(&seed.orig);
                        if unproven_path.is_none() {
                            unproven_path = Some(seed.orig.to_string());
                        }
                    }
                }
            }
            if proven.is_none() && unproven_path.is_none() {
                continue; // every seeding prop provably never changes
            }

            let escaped = setter_escapes(comp, &slot_setters);
            let slot_name = name_of(*label);

            let (mut severity, message, proven_evidence) = match (&proven, &unproven_path) {
                (Some((path, slot, display, write_span)), _) => {
                    let severity = if escaped {
                        // Something outside this component holds the setter —
                        // an unseen sync path may exist.
                        Severity::Warning
                    } else {
                        Severity::Error
                    };
                    (
                        severity,
                        format!(
                            "state {slot_name} is seeded from `{path}`, which is fed by \
                             {display} and changes — `useState` reads its initializer on the \
                             first render only and nothing here re-syncs it, so {slot_name} \
                             stays frozen at the first `{path}` value"
                        ),
                        Some((*slot, display.clone(), *write_span)),
                    )
                }
                (None, Some(path)) => (
                    Severity::Warning,
                    format!(
                        "state {slot_name} is seeded from `{path}` and never re-synced — \
                         `useState` reads its initializer on the first render only, so if \
                         `{path}` changes, {slot_name} keeps the mount-time value"
                    ),
                    None,
                ),
                (None, None) => unreachable!("guarded above"),
            };

            // Seed-once intent named on every seeding prop (`initialValue`)
            // downgrades one level: the pattern is idiomatic, the finding
            // stays visible.
            if all_seed_named {
                severity = match severity {
                    Severity::Error => Severity::Warning,
                    _ => Severity::Info,
                };
            }

            // Local slot never written (setter never even referenced —
            // `const [{ snap }] = useState(...)`): the author never intended
            // this state to move, it is a deliberate mount-time snapshot.
            // Surfaced as advice only.
            if !locally_written.contains(label) {
                severity = Severity::Info;
            }

            let primary_path = proven
                .as_ref()
                .map(|(p, _, _, _)| p.clone())
                .or(unproven_path)
                .unwrap_or_default();

            let mut d = Diagnostic::new("frozen-initial-state", message)
                .with_severity(severity)
                .with_label(*label)
                .with_var(primary_path.clone());
            if let Some(r) = span {
                d = d.with_range(*r);
            }
            // Witness (ADR-019): the prop read at the seed site, the
            // init-once semantics, and — when proven — the write that moves
            // the feeding slot in its owner.
            d = d.with_step(
                Step::Read {
                    what: primary_path.clone(),
                },
                Some(*label),
                *span,
                &name_of,
            );
            d = d.with_step(
                Step::InitOnce { slot: *label },
                Some(*label),
                *span,
                &name_of,
            );
            if let Some((owner_slot, display, write_span)) = proven_evidence {
                let owner_name = move |_: HookLabel| display.clone();
                d = d.with_step(
                    Step::Write {
                        slot: owner_slot,
                        value: ValueClass::Unknown,
                    },
                    None,
                    write_span,
                    &owner_name,
                );
            }
            diags.push(d);
        }

        diags
    }
}

/// Like [`eval_in_exit_env`] but with the component's converged heap, so a
/// props-param-rooted `FieldAccess` resolves through the props `Obj` instead
/// of degrading to ⊤.
fn eval_with_heap(expr: &Expr, comp: &AnalysisResult<StateValue>) -> StateValue {
    // Same eval core as `eval_in_exit_env`, but seeded from the component's
    // converged heap (`comp.heap.clone()`) instead of an empty one, so a
    // props-param-rooted `FieldAccess` resolves through the props `Obj` rather
    // than degrading to ⊤. The heap choice is exactly what distinguishes this
    // wrapper — it must stay comp-seeded here.
    use super::ConvergedEval;
    comp.eval_in(&comp.exit_env(), expr, &mut comp.heap.clone())
}
