use crate::ir::hooks::HookEntry;
use crate::rules::helpers::join_names;
use crate::rules::{
    Diagnostic, OptionKind, OptionSpec, Rule, RuleCtx, Step, state_slot_name, state_val_labels,
};

/// Fires when a state slot is used only inside one child subtree of its
/// owner: every component from the owner down to that subtree re-renders on
/// each write only to hand the value (or its setter) on.
///
/// "Used" is the render dependence of [`crate::engine::render_deps`]: the
/// value or the setter reaches a component's host output, an effect, a
/// render-phase side effect or an element's mount condition, as opposed to
/// being forwarded as a prop. The descent stops at anything it cannot see
/// into (an unresolved element, a list, recursion), so a finding is a proof
/// that the components above the home do not use the slot.
///
/// Warning, never Error: the extra re-renders are certain, what they cost is
/// not.
pub struct StateLiftedTooHigh;

impl StateLiftedTooHigh {
    const NAME: &'static str = "state-lifted-too-high";
    const MIN_DEPTH: OptionSpec = OptionSpec {
        name: "minDepth",
        kind: OptionKind::UInt {
            default: 1,
            min: 1,
            max: 64,
        },
        doc: "report only when the state's home is at least this many levels below its owner",
    };
    const MIN_WASTED: OptionSpec = OptionSpec {
        name: "minWastedRenders",
        kind: OptionKind::UInt {
            default: 2,
            min: 1,
            max: 1000,
        },
        doc: "report only when each write re-renders at least this many components for \
              nothing (the components above the home plus the other elements they build)",
    };
}

impl Rule for StateLiftedTooHigh {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn options(&self) -> &'static [OptionSpec] {
        &[Self::MIN_DEPTH, Self::MIN_WASTED]
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let program = ctx.program();
        let owner = ctx.component();
        let Some(result) = program.components.get(&owner) else {
            return Vec::new();
        };
        let min_depth = ctx.config().uint(&Self::MIN_DEPTH) as usize;
        let min_wasted = ctx.config().uint(&Self::MIN_WASTED) as usize;
        let index = ctx.cache().render();
        let names = state_val_labels(&result.render_cfg);
        let mut diags = Vec::new();
        for hook in &result.hooks {
            let HookEntry::State { label, span, .. } = hook else {
                continue;
            };
            let Some(home) = index.home_of(owner, *label, program) else {
                continue;
            };
            if home.path.is_empty()
                || home.path.len() < min_depth
                || home.wasted_renders() < min_wasted
            {
                continue;
            }
            let slot = state_slot_name(*label, &names);
            let target = program.display_name(home.path.last().expect("non-empty").to);
            let mut rerendered: Vec<String> = home
                .path
                .iter()
                .map(|h| format!("`{}`", program.display_name(h.from)))
                .collect();
            let levels = home.path.len();
            let pronoun = if levels == 1 {
                "it renders"
            } else {
                "they render"
            };
            match home.siblings {
                0 => {}
                1 => rerendered.push(format!("1 other component {pronoun}")),
                n => rerendered.push(format!("{n} other components {pronoun}")),
            }
            let home_id = home.path.last().expect("non-empty").to;
            // A component rendered from several places is shared: moving the
            // state into it would change it for every other caller, so the
            // advice is a wrapper that owns the state and renders it.
            let fix = if index.mount_count(home_id) > 1 {
                format!(
                    "`{target}` is rendered from other places too, so wrap this \
                     `<{target}>` in a small component that owns the state"
                )
            } else {
                format!("move the state into `{target}`")
            };
            // Reached by context, the state takes its provider along, and the
            // components on the way do not even pass it down.
            let context = home.path.iter().find_map(|h| h.context.as_deref());
            let (why, fix) = match context {
                Some(ctx) => (
                    "although none of them uses it",
                    format!("{fix}, with the `{ctx}` provider that hands it on"),
                ),
                None => ("only to pass it down", fix),
            };
            let message = format!(
                "state {slot} is only used inside `<{target}>`, {levels} level{} below `{}`. \
                 Every write re-renders {} {why}; {fix}",
                if levels == 1 { "" } else { "s" },
                program.display_name(owner),
                join_names(&rerendered),
            );
            let mut d = Diagnostic::warn(Self::NAME, message).with_label(*label);
            if let Some(r) = span {
                d = d.with_range(*r);
            }
            for hop in &home.path {
                d = d.with_step(
                    Step::Forward {
                        from: program.display_name(hop.from),
                        to: program.display_name(hop.to),
                        props: hop.props.clone().unwrap_or_default(),
                        context: hop.context.clone(),
                    },
                    None,
                    hop.span,
                    &|l| state_slot_name(l, &names),
                );
            }
            diags.push(d);
        }
        diags
    }
}
