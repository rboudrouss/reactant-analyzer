use crate::rules::RuleCtx;

use crate::{
    domains::StateValue,
    engine::{AnalysisResult, SeedSync, SlotSeed},
    ir::{
        expr::Expr,
        free_vars::AccessPath,
        hooks::HookEntry,
        types::{HookLabel, Var},
    },
};

use crate::rules::{
    Certified, Diagnostic, Motion, MovingFeeder, MustResult, Rule, Severity, Step, ValueClass,
    all_setter_labels, classify_motion, helpers::mount::MountCoupling, may_written_slots,
    must_frozen_seed, state_slot_name, state_val_labels,
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
///   render-time write) syncs the local slot, the local setter never escapes
///   this component (nothing else could sync it), and the move is *observable*
///   — no call site ties the mount condition to the feeder's own writers.
/// - **Warning**: the freeze is real but the prop's motion is not proven
///   (props ⊤ in intra-only analysis, `VersionedTop`, per-render or unknown
///   values), or the proof chain has a hole (setter escapes, feeding slot's
///   owner unavailable, mount writer-coupled).
/// - **Info**: intent is declared — every seeding prop is *named* for
///   seed-once (`initial*` / `default*`) with unproven motion, the local slot
///   is never written at all (`const [{ snap }] = useState(...)`: a deliberate
///   mount-time snapshot), or every call site re-seeds the consumer when the
///   prop moves (`key={seed}`, or a render guarded by the seed —
///   [`MountCoupling::Reseeds`], issue #95) — surfaced as advice only.
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

/// `initialValue` / `defaultTab` — the prop's own name declares seed-once
/// intent (uncontrolled-with-default idiom).
fn is_seed_named(p: &AccessPath) -> bool {
    let name = p.segments.last().map(String::as_str).unwrap_or(&p.root);
    let lower = name.to_ascii_lowercase();
    lower.starts_with("initial") || lower.starts_with("default")
}

impl FrozenInitialState {
    const NAME: &'static str = "frozen-initial-state";
}

impl Rule for FrozenInitialState {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        let comp = result.components.get(component)?;
        (!comp.slot_seeds.is_empty()).then_some(crate::rules::SafeCheck {
            rule: Self::NAME,
            message: "no state slot freezes a changing prop's first value",
        })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let comp = &result.components[component];
        let render_cfg = &comp.render_cfg;
        let setter_labels = all_setter_labels(comp);
        let state_labels = state_val_labels(render_cfg);
        let locally_written = may_written_slots(render_cfg, &comp.hooks, &setter_labels);
        let name_of = |l: HookLabel| state_slot_name(l, &state_labels);

        let mut diags = Vec::new();

        for hook in &comp.hooks {
            let HookEntry::State { label, span, .. } = hook else {
                continue;
            };
            // The seed and sync halves come from the engine relation (#106,
            // ADR-031): the rule no longer runs its own scan of either. What
            // stays here is everything the relation deliberately does not
            // carry — the moving-feeder proof, the Info strata, and the #95
            // mount-coupling downgrade.
            let seeds: Vec<&SlotSeed> = comp.seeds_of(*label).collect();
            if seeds.is_empty() {
                continue;
            }
            // A render-time write, or an effect that re-runs when this prop
            // moves: a sync path exists. Its quality is `derived-state`'s
            // business, and the render-time case belongs to `setter-in-render`.
            if seeds.iter().any(|s| s.sync == SeedSync::Synced) {
                continue;
            }

            // Classify each seeding prop; keep the strongest surviving verdict.
            // proven = (seed path, certified moving feeder) — the proof is
            // minted by `classify_motion` at the point of knowledge (ADR-021).
            let mut proven: Option<(String, Certified<MovingFeeder>)> = None;
            let mut unproven: Option<crate::ir::free_vars::AccessPath> = None;
            let mut all_seed_named = true;
            // Prop names the moving seeds arrive through — what the call sites
            // must re-seed for the freeze to be unobservable. `None` once a
            // moving seed reads the props object itself (`useState(props)`):
            // no single prop carries it, so no call site can prove anything.
            let mut moving_props: Option<Vec<Var>> = Some(Vec::new());
            let note_moving = |seed: &SlotSeed, moving_props: &mut Option<Vec<Var>>| {
                let Some(names) = moving_props else { return };
                for n in &seed.normalized {
                    match n.segments.first() {
                        Some(prop) => names.push(prop.clone()),
                        None => {
                            *moving_props = None;
                            return;
                        }
                    }
                }
            };
            for seed in &seeds {
                let mut expr = Expr::Var(seed.path.root.clone());
                for seg in &seed.path.segments {
                    expr = Expr::FieldAccess {
                        obj: Box::new(expr),
                        field: seg.clone(),
                    };
                }
                let val = eval_with_heap(&expr, comp);
                match classify_motion(&val, result) {
                    Motion::Still => continue,
                    Motion::Proven(proof) => {
                        all_seed_named &= is_seed_named(&seed.path);
                        note_moving(seed, &mut moving_props);
                        if proven.is_none() {
                            proven = Some((seed.path.to_string(), proof));
                        }
                    }
                    Motion::Unproven => {
                        all_seed_named &= is_seed_named(&seed.path);
                        note_moving(seed, &mut moving_props);
                        // Several members of one object can seed the same
                        // slot (`const input = action.settings.input`, then a
                        // dozen reads of it): name the handle they share.
                        unproven = Some(match &unproven {
                            Some(prev) => prev.common_prefix(&seed.path),
                            None => seed.path.clone(),
                        });
                    }
                }
            }
            let unproven_path = unproven.map(|p| p.to_string());
            if proven.is_none() && unproven_path.is_none() {
                continue; // every seeding prop provably never changes
            }

            // Mount lifetime (issue #95): a prop that moves only freezes a
            // consumer that stays mounted across the move.
            let mount = match &moving_props {
                Some(names) => ctx.cache().mounts().coupling(
                    component,
                    names,
                    proven
                        .as_ref()
                        .map(|(_, p)| (&p.evidence().owner, p.evidence().slot)),
                    result,
                ),
                None => MountCoupling::Free,
            };

            // Every row of one slot carries the same escape verdict — it is a
            // property of the setter, not of the seed.
            let escaped = seeds[0].setter_escapes;
            let slot_name = name_of(*label);

            let (mut severity, message, proven_evidence) = match (&proven, &unproven_path) {
                (Some((path, proof)), _) => {
                    let feeder = proof.evidence();
                    // `escaped`: something outside this component holds the
                    // setter — an unseen sync path may exist. `WriterCoupled`:
                    // the mount condition moves with the feeder, so no mounted
                    // instance need ever observe the change.
                    let severity = if escaped || mount == MountCoupling::WriterCoupled {
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
                             stays frozen at the first `{path}` value",
                            display = feeder.display
                        ),
                        Some((feeder.slot, feeder.display.clone(), feeder.write_span)),
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

            // Every call site re-seeds on change (`key={seed}`, or a render
            // guarded by the seed): the freeze is real but expected to arrive
            // on a fresh instance. Advice only — and never a kill, since
            // neither shape proves the child cannot survive the change.
            if mount == MountCoupling::Reseeds {
                severity = Severity::Info;
            }

            let primary_path = proven
                .as_ref()
                .map(|(p, _)| p.clone())
                .or(unproven_path)
                .unwrap_or_default();

            // The Error tier consumes the `Certified<MovingFeeder>` minted by
            // `classify_motion` (the point of knowledge); `must_frozen_seed`
            // demotes it when an idiomatic downgrade applies, so `severity ==
            // Error` ⟹ `All`. Warning/Info are safe over-claims.
            let feeder_proof = proven.map(|(_, proof)| proof);
            let mut d = match (severity, feeder_proof) {
                (Severity::Error, Some(proof)) => match must_frozen_seed(
                    proof,
                    escaped,
                    all_seed_named,
                    locally_written.contains(label),
                    mount,
                ) {
                    MustResult::All(proof) => {
                        Diagnostic::error("frozen-initial-state", proof, message)
                    }
                    _ => Diagnostic::warn("frozen-initial-state", message),
                },
                (Severity::Info, _) => Diagnostic::info("frozen-initial-state", message),
                _ => Diagnostic::warn("frozen-initial-state", message),
            }
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
    use crate::rules::ConvergedEval;
    comp.eval_in(&comp.exit_env(), expr, &mut comp.heap.clone())
}
