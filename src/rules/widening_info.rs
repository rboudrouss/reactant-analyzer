use crate::{domains::StateValue, engine::AnalysisResult};

use super::{Diagnostic, Rule, Severity};

/// Emits an Info diagnostic for each state label that required widening to
/// force fixpoint convergence.  Widening = over-approximation → abstract
/// values are less precise.  Not an error, but useful context when --info.
pub struct WideningInfo;

impl Rule for WideningInfo {
    fn name(&self) -> &'static str {
        "widening-info"
    }

    fn check(&self, result: &AnalysisResult<StateValue>) -> Vec<Diagnostic> {
        let mut labels: Vec<_> = result.widened_labels.iter().copied().collect();
        labels.sort_unstable();
        labels
            .into_iter()
            .map(|label| {
                Diagnostic::new(
                    "widening-info",
                    format!(
                        "state {label} required widening to force convergence \
                         — abstract values are over-approximated"
                    ),
                )
                .with_severity(Severity::Info)
            })
            .collect()
    }
}
