use std::collections::BTreeSet;

use crate::engine::WriterRegion;
use crate::engine::render_deps::param_gated_vars;
use crate::ir::free_vars::compute_free_vars;
use crate::ir::hooks::HookEntry;
use crate::ir::{ComponentId, HookLabel, SourceRange, expr::Expr};
use crate::rules::helpers::join_names;
use crate::rules::helpers::render_tree::{Frequency, event_frequency};
use crate::rules::{
    Diagnostic, OptionKind, OptionSpec, Rule, RuleCtx, Step, state_slot_name, state_val_labels,
};

/// Fires when a state written from a continuous event (typing, pointer
/// motion, scrolling, dragging, a timer) lives in a component that also
/// builds elements none of whose inputs depend on it: every write re-renders
/// those subtrees for identical output.
///
/// The state itself is used where it lives (otherwise
/// `state-lifted-too-high` names where it belongs). The fix is structural:
/// move the state and the part of the output that uses it into their own
/// component, or build the heavy subtree higher up and pass it in as
/// `children`, whose element identity React keeps.
///
/// An element is a candidate only when it names a component the analysis
/// resolved (an unresolved one may be a `memo` barrier) and nothing it is
/// nested in receives the state (a provider could carry it down by context).
/// Warning: the extra renders are certain, their cost is not.
pub struct WastedSubtreeRender;

impl WastedSubtreeRender {
    const NAME: &'static str = "wasted-subtree-render";
    const MIN_WASTED: OptionSpec = OptionSpec {
        name: "minWastedRenders",
        kind: OptionKind::UInt {
            default: 2,
            min: 1,
            max: 1000,
        },
        doc: "report only when each write re-renders at least this many components for \
              nothing (a subtree holding a list always qualifies)",
    };
    const CONTINUOUS_ONLY: OptionSpec = OptionSpec {
        name: "continuousOnly",
        kind: OptionKind::Bool { default: true },
        doc: "report only states written from a continuous event (typing, pointer motion, \
              scroll, drag, a timer); `false` also reports clicks and other discrete events",
    };
}

impl Rule for WastedSubtreeRender {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn options(&self) -> &'static [OptionSpec] {
        &[Self::MIN_WASTED, Self::CONTINUOUS_ONLY]
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let program = ctx.program();
        let owner = ctx.component();
        let Some(result) = program.components.get(&owner) else {
            return Vec::new();
        };
        let min_wasted = ctx.config().uint(&Self::MIN_WASTED) as usize;
        let continuous_only = ctx.config().flag(&Self::CONTINUOUS_ONLY);
        let index = ctx.cache().render();
        let names = state_val_labels(&result.render_cfg);
        let states: BTreeSet<HookLabel> = result
            .hooks
            .iter()
            .filter_map(|h| match h {
                HookEntry::State { label, .. } => Some(*label),
                _ => None,
            })
            .collect();
        // One trigger (an event handler, or an effect's registered callbacks)
        // writes its slots in one batch, so they re-render together: group
        // by it.
        let mut triggers: Vec<Trigger> = Vec::new();
        let mut add = |key: TriggerKey, slot: HookLabel, events: Events, span, place| {
            let i = match triggers.iter().position(|t| t.key == key) {
                Some(i) => i,
                None => {
                    triggers.push(Trigger {
                        key,
                        slots: BTreeSet::new(),
                        events: BTreeSet::new(),
                        span,
                        place,
                    });
                    triggers.len() - 1
                }
            };
            triggers[i].slots.insert(slot);
            triggers[i].events.extend(events);
        };
        // A handler may be the owner's own, or one down the tree that the
        // setter was handed to: either way the write re-renders the owner.
        for &slot in &states {
            for l in index.landings(owner, slot, program) {
                let f = event_frequency(&l.event, Some(&l.target), l.keyed);
                let events =
                    BTreeSet::from([(l.event.to_ascii_lowercase(), f == Frequency::Continuous)]);
                let elsewhere = l.component != owner;
                let (span, place) = if elsewhere {
                    (l.via, Some(l.component))
                } else {
                    (l.span, None)
                };
                add(
                    TriggerKey::Handler(l.via, l.component, l.span, l.event),
                    slot,
                    events,
                    span,
                    place,
                );
            }
        }
        for w in &result.slot_writers {
            if !states.contains(&w.slot) {
                continue;
            }
            // The registrations of the effect whose callback may write
            // through this setter.
            let WriterRegion::Effect(e) = w.region else {
                continue;
            };
            let events: Events = result
                .registrations
                .iter()
                .filter(|r| r.effect == e && may_call(&r.callback, &w.setter))
                .map(|r| {
                    let ev = r.event.clone().unwrap_or_else(|| r.registrar.to_string());
                    let f = event_frequency(&ev, None, keyed_call(&r.callback, &w.setter));
                    (ev.to_ascii_lowercase(), f == Frequency::Continuous)
                })
                .collect();
            let span = result.effect_info.get(&e).and_then(|i| i.span);
            add(TriggerKey::Effect(e), w.slot, events, span, None);
        }
        let mut diags = Vec::new();
        for Trigger {
            key,
            slots,
            events,
            span,
            place,
        } in triggers
        {
            // A continuous event re-renders continuously only if it can keep
            // writing new values: a boolean or a few constants re-render at
            // the rate of their transitions (`scrollY > 50` flips once).
            let many_valued = slots
                .iter()
                .any(|l| !result.state_store.get(*l).is_finitely_valued());
            let continuous: Vec<&String> = events
                .iter()
                .filter(|(_, c)| *c && many_valued)
                .map(|(e, _)| e)
                .collect();
            if events.is_empty() || (continuous_only && continuous.is_empty()) {
                continue;
            }
            let labels: Vec<HookLabel> = slots.iter().copied().collect();
            let wasted = index.wasted_siblings(owner, &labels, program);
            let total: usize = wasted.iter().map(|w| w.renders).sum();
            let list = wasted.iter().any(|w| w.list);
            if wasted.is_empty() || (total < min_wasted && !list) {
                continue;
            }
            let slot_names: Vec<String> =
                labels.iter().map(|l| state_slot_name(*l, &names)).collect();
            let slot = join_names(&slot_names);
            let noun = if labels.len() == 1 { "state" } else { "states" };
            let event = continuous
                .first()
                .copied()
                .or_else(|| events.iter().next().map(|(e, _)| e))
                .expect("a trigger has an event");
            let trigger = match place {
                Some(c) => format!(
                    "each `{event}` event in `<{}>` writes {noun} {slot} and",
                    program.display_name(c)
                ),
                None => format!("each `{event}` event writes {noun} {slot} and"),
            };
            let label = &labels[0];
            let elements: Vec<String> = wasted
                .iter()
                .map(|w| format!("`<{}>`", program.display_name(w.child)))
                .collect();
            let cost = if list {
                format!("at least {total} component renders, including a list")
            } else {
                format!("{total} component renders")
            };
            let pronoun = if labels.len() == 1 { "it" } else { "them" };
            // A trigger spliced in from a custom hook is the hook's to fix:
            // every component calling it pays the same renders.
            let hook = match key {
                TriggerKey::Effect(e) => region_hook(program, result, e),
                TriggerKey::Handler(..) => None,
            };
            let fix = match hook {
                Some(hook) => format!(
                    "The writes come from `{hook}`, which does this to every component that \
                     calls it: write only when a value its callers read changes, or call it \
                     from a smaller component"
                ),
                None => format!(
                    "Move {slot} and the elements that use {pronoun} into their own component, \
                     or build the unrelated elements higher up and pass them in as `children`"
                ),
            };
            let message = format!(
                "{trigger} re-renders {} ({cost}) although none of {} inputs depends on \
                 {pronoun}. {fix}",
                join_names(&elements),
                if wasted.len() == 1 { "its" } else { "their" },
            );
            let mut d = Diagnostic::warn(Self::NAME, message).with_label(*label);
            if let Some(r) = span {
                d = d.with_range(r);
            }
            for w in &wasted {
                d = d.with_step(
                    Step::Rerender {
                        component: program.display_name(w.child),
                        renders: w.renders,
                        list: w.list,
                    },
                    None,
                    w.span,
                    &|l| state_slot_name(l, &names),
                );
            }
            diags.push(d);
        }
        diags
    }
}

type Events = BTreeSet<(String, bool)>;

/// What makes two writes one batch.
#[derive(PartialEq)]
enum TriggerKey {
    /// One event handler: the owner's element the setter leaves through (two
    /// instances of one child are two handlers), the component building the
    /// handler's element, the handler prop's span, the event.
    Handler(
        Option<SourceRange>,
        ComponentId,
        Option<SourceRange>,
        String,
    ),
    /// The listeners and timers one effect registers.
    Effect(HookLabel),
}

struct Trigger {
    key: TriggerKey,
    slots: BTreeSet<HookLabel>,
    /// Each event, and whether it is continuous.
    events: Events,
    /// Where the finding points: in the owner.
    span: Option<SourceRange>,
    /// The component the handler sits in, when not the owner.
    place: Option<ComponentId>,
}

/// The custom hook an effect was inlined from, by name: the effect's inlined
/// provenance row is positioned in the hook's file, and the component's own
/// call of a non-React hook resolved to that file names it.
fn region_hook(
    program: &crate::engine::ProgramAnalysisResult,
    result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    label: HookLabel,
) -> Option<String> {
    let at = result
        .hook_provenance
        .iter()
        .find(|p| p.label == label && p.inlined)?
        .span?;
    let file = program.file_table.path(at.file)?;
    result
        .hook_provenance
        .iter()
        .find(|p| !p.inlined && !p.react && p.file.as_deref() == Some(file))
        .map(|p| p.origin_hook.clone())
}

/// Whether a registered callback may call `setter`: an inline function that
/// reads it, or anything else (a named listener this walk does not follow).
fn may_call(callback: &Expr, setter: &str) -> bool {
    match callback.peel_ts() {
        Expr::FnLit { body_cfg, .. } => compute_free_vars(body_cfg).contains(setter),
        _ => true,
    }
}

/// Whether a registered callback calls `setter` only behind a test of its
/// event argument.
fn keyed_call(callback: &Expr, setter: &str) -> bool {
    match callback.peel_ts() {
        Expr::FnLit {
            params, body_cfg, ..
        } => param_gated_vars(params, body_cfg).contains(setter),
        _ => false,
    }
}
