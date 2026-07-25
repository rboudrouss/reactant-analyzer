use super::RuleCtx;

use super::{Diagnostic, Rule};

/// Emits an Info diagnostic for each state label that required widening to
/// force fixpoint convergence.  Widening = over-approximation → abstract
/// values are less precise.  Not an error, but useful context when --info.
pub struct WideningInfo;

impl Rule for WideningInfo {
    fn name(&self) -> &'static str {
        "widening-info"
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let result = &result.components[component];
        let mut labels: Vec<_> = result.widen_trace.keys().copied().collect();
        labels.sort_unstable();
        labels
            .into_iter()
            .map(|label| {
                Diagnostic::info(
                    "widening-info",
                    format!(
                        "state {label} kept changing during analysis and was \
                         approximated to converge — findings that depend on it \
                         may be imprecise"
                    ),
                )
                // Witness (ADR-019): the engine's own record of the widening.
                .with_notes(super::witness::slot_history(
                    result,
                    label,
                    &super::witness::fallback_name,
                ))
            })
            .collect()
    }
}
