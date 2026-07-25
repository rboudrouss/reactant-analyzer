use super::RuleCtx;
use crate::{engine::HookKind, ir::hooks::HookEntry};

use super::{Diagnostic, Rule};

/// Emits `Info` diagnostics when the analyser deliberately truncates analysis
/// to preserve soundness.  Each site is a potential source of false negatives.
///
/// Four cases:
/// - `recursion-cutoff`    component references itself (directly or transitively);
///                           the recursive call is resolved to ⊤.
/// - `unknown-component`   component instantiates a child not found in the
///                           analysis registry (imported from an unanalyzed file);
///                           props and effects of that child are treated as ⊤.
/// - `callback-depth-cap`  callback inlining reached MAX_INLINE_DEPTH; deeper
///                           HOF chains (`.then(() => .then(…))`) not descended.
/// - `unknown-hook`        custom hook call whose source is not in the registry
///                           and has no `HookSummary`; its internals are opaque (FN possible).
pub struct AnalysisLimitInfo;

impl Rule for AnalysisLimitInfo {
    fn name(&self) -> &'static str {
        "analysis-limit"
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

        // Unknown custom hooks survived expand_custom_hooks (not in HookRegistry or SummaryRegistry).
        if let Some(comp_result) = result.components.get(component) {
            for call in &comp_result.hook_calls {
                if call.kind != HookKind::Custom {
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

        diags
    }
}
