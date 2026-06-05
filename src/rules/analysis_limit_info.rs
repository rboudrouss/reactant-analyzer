use crate::{engine::ProgramAnalysisResult, ir::types::Symbol};

use super::{Diagnostic, Rule, Severity};

/// Emits `Info` diagnostics when the analyser deliberately truncates analysis
/// to preserve soundness.  Each site is a potential source of false negatives.
///
/// Three cases:
/// - `recursion-cutoff`    — component references itself (directly or transitively);
///                           the recursive call is resolved to ⊤.
/// - `unknown-component`   — component instantiates a child not found in the
///                           analysis registry (imported from an unanalyzed file);
///                           props and effects of that child are treated as ⊤.
/// - `callback-depth-cap`  — callback inlining reached MAX_INLINE_DEPTH; deeper
///                           HOF chains (`.then(() => .then(…))`) not descended.
pub struct AnalysisLimitInfo;

impl Rule for AnalysisLimitInfo {
    fn name(&self) -> &'static str {
        "analysis-limit"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let mut diags = vec![];
        let stats = &result.stats;

        for (caller, callee) in &stats.recursive_component_refs {
            if caller == component {
                diags.push(
                    Diagnostic::new(
                        "analysis-limit",
                        format!(
                            "recursive component reference `{callee}` cut to ⊤ — \
                             cross-component cycles not fully analysed (FN possible)"
                        ),
                    )
                    .with_severity(Severity::Info),
                );
            }
        }

        for (caller, callee) in &stats.unknown_component_refs {
            if caller == component {
                diags.push(
                    Diagnostic::new(
                        "analysis-limit",
                        format!(
                            "component `{callee}` not found in analysis registry — \
                             pass its file on the command line to analyse it (FN possible)"
                        ),
                    )
                    .with_severity(Severity::Info),
                );
            }
        }

        if stats.callback_depth_capped.contains(component) {
            diags.push(
                Diagnostic::new(
                    "analysis-limit",
                    format!(
                        "callback inlining reached depth cap ({}) — \
                         deeper HOF chains not descended (FN possible on nested callbacks)",
                        crate::domains::interp::MAX_INLINE_DEPTH
                    ),
                )
                .with_severity(Severity::Info),
            );
        }

        diags
    }
}
