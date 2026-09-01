use crate::ir::hooks::HookEntry;
use crate::rules::RuleCtx;

use crate::rules::{Diagnostic, Rule};

/// Emits `Info` diagnostics when the analyser deliberately truncates analysis
/// to preserve soundness.  Each site is a potential source of false negatives.
///
/// Five cases:
/// - `recursion-cutoff` — component references itself (directly or transitively);
///   the recursive call is resolved to ⊤.
/// - `unknown-component` — component instantiates a child not found in the
///   analysis registry (imported from an unanalyzed file); props and effects of
///   that child are treated as ⊤.
/// - `callback-depth-cap` — callback inlining reached MAX_INLINE_DEPTH; deeper
///   HOF chains (`.then(() => .then(…))`) not descended.
/// - `unknown-hook` — custom hook call whose source is not in the registry and
///   has no `HookSummary`; its internals are opaque (FN possible).
/// - `inline-budget` — utility inlining used up `Config::max_inline_depth`
///   splices for this component; the utility calls still standing stay ⊤.
pub struct AnalysisLimitInfo;

impl AnalysisLimitInfo {
    pub(crate) const NAME: &'static str = "analysis-limit";
}

impl Rule for AnalysisLimitInfo {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let mut diags = vec![];
        let stats = &result.stats;

        for (caller, callee) in &stats.recursive_component_refs {
            if caller == component {
                diags.push(Diagnostic::info(
                    "analysis-limit",
                    format!(
                        "recursive component reference `{callee}` not followed — \
                             its props are treated as unknown; cross-component cycles \
                             are not fully analysed (FN possible)"
                    ),
                ));
            }
        }

        for (caller, callee) in &stats.unknown_component_refs {
            if caller == component {
                diags.push(Diagnostic::info(
                    "analysis-limit",
                    format!(
                        "component `{callee}` not found in analysis registry \
                             pass its file on the command line to analyse it (FN possible)"
                    ),
                ));
            }
        }

        if stats.callback_depth_capped.contains(component) {
            diags.push(Diagnostic::info(
                "analysis-limit",
                format!(
                    "callback inlining reached depth cap ({}) \
                         deeper HOF chains not descended (FN possible on nested callbacks)",
                    crate::domains::interp::MAX_INLINE_DEPTH
                ),
            ));
        }

        if stats.inline_budget_exhausted.contains(component) {
            diags.push(Diagnostic::info(
                "analysis-limit",
                "utility inlining ran out of splice budget here — the remaining \
                 utility calls are treated as unknown (FN possible); raise \
                 `max_inline_depth` to inline more",
            ));
        }

        // Unknown custom hooks survived expand_custom_hooks (not in HookRegistry or SummaryRegistry).
        if let Some(comp_result) = result.components.get(component) {
            for call in &comp_result.hook_calls {
                // Not `kind == Custom`: a summarized library hook is a custom
                // hook whose abstraction is known, and it keeps its row so
                // rules-of-hooks checks can see the call site. `opaque` is the
                // fact this Info is about.
                if !call.opaque {
                    continue;
                }
                let name = comp_result.hooks.iter().find_map(|h| match h {
                    HookEntry::Custom { label, name, .. } if *label == call.label => {
                        Some(name.clone())
                    }
                    _ => None,
                });
                let name = name.unwrap_or_else(|| format!("<hook:{}>", call.label));
                let mut d = Diagnostic::info(
                    "analysis-limit",
                    format!(
                        "hook `{name}` not found in registry \
                         pass its source file or add a HookSummary to analyse it (FN possible)"
                    ),
                )
                .with_label(call.label);
                if let Some(span) = call.span {
                    d = d.with_range(span);
                }
                diags.push(d);
            }
        }

        // A deps argument the engine could not read (`useMemo(fn, deps)`). The
        // hook is gated by a list of which not one element is visible, so
        // every deps-based rule is running blind on it — and `registry.rs`
        // keys the suspension of "verified:" assurances off this Info, which
        // is how a component stops publishing a universal over a hook nobody
        // could check.
        if let Some(comp_result) = result.components.get(component) {
            for info in comp_result.effect_info.values() {
                if !info.deps_are_opaque() {
                    continue;
                }
                let mut d = Diagnostic::info(
                    "analysis-limit",
                    "the deps argument here is not a written array, so its entries \
                     cannot be enumerated — deps checks run with nothing declared \
                     (FP possible, and FN on whatever the list does gate)",
                )
                .with_label(info.label);
                if let Some(span) = info.span {
                    d = d.with_range(span);
                }
                diags.push(d);
            }
        }

        diags
    }
}
