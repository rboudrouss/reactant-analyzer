use crate::rules::RuleCtx;

use crate::rules::{Diagnostic, Rule, state_slot_name, state_val_labels};

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
        let result = &result.components[&component];
        let state_names = state_val_labels(&result.render_cfg);
        let name_of = |l| state_slot_name(l, &state_names);
        let mut labels: Vec<_> = result.widen_trace.keys().copied().collect();
        labels.sort_unstable();
        labels
            .into_iter()
            .map(|label| {
                Diagnostic::info(
                    "widening-info",
                    format!(
                        "state {} kept changing during analysis and was \
                         approximated to converge, so findings that depend on it \
                         may be imprecise",
                        name_of(label)
                    ),
                )
                // Witness (ADR-019): the engine's own record of the widening.
                .with_notes(crate::rules::api::witness::slot_history(
                    result, label, &name_of,
                ))
            })
            .collect()
    }
}
