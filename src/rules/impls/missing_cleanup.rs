use crate::engine::registrations::Firing;
use crate::ir::hooks::HookEntry;
use crate::rules::api::query::{CleanupVerdict, cleanup_verdict};
use crate::rules::{Diagnostic, Rule, RuleCtx};

/// Fires when a `useEffect` starts something that keeps running — an interval,
/// an event listener, a subscription — and returns no teardown.
///
/// React runs an effect again whenever its deps change, and once more on
/// unmount; under StrictMode it runs it twice on the first mount. Each run
/// registers again, and without a cleanup nothing ever unregisters: the
/// handlers accumulate, and the ones from earlier renders keep firing against
/// state the component no longer has.
///
/// Two decisions bound the noise, both about what the advice is worth:
///
/// - **Repeating registrars only.** `setInterval`, `addEventListener`,
///   `subscribe`, `on`, `addListener` keep firing until torn down, so their
///   absence is a leak. A one-shot `setTimeout` or `.then` at worst fires late
///   against an unmounted component — a real problem, but a different one
///   (an abort flag, not a teardown), and firing here on every promise chain
///   inside an effect would bury the signal.
/// - **A provable absence only.** [`CleanupVerdict`] is three-valued and
///   `Unknown` folds to "there may be a cleanup". The rule acts on `Absent`
///   alone: an effect that returns *something* has an author who wrote a
///   teardown, or wrote something unclassifiable, and the advice is wrong in
///   both cases.
///
/// Warning, never Error: the registration and the teardown can both be real
/// and still live in a helper this rule cannot see through.
pub struct MissingCleanup;

impl MissingCleanup {
    pub(crate) const NAME: &'static str = "missing-cleanup";
}

impl Rule for MissingCleanup {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        use crate::engine::HookKind;
        crate::rules::has_hook_kind(ctx.program(), ctx.component(), HookKind::Effect).then_some(
            crate::rules::SafeCheck {
                rule: Self::NAME,
                message: "every effect that starts something long-lived also tears it down",
            },
        )
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let result = ctx.comp();
        let mut diags = Vec::new();

        for hook in &result.hooks {
            let HookEntry::Effect {
                label, body_cfg, ..
            } = hook
            else {
                continue;
            };
            if cleanup_verdict(body_cfg) != CleanupVerdict::Absent {
                continue;
            }

            // The engine's registration relation (ADR-034), never a second
            // scan of the same bodies.
            // Deterministic: the scan walks blocks in id order, but two
            // registrations in one block tie — order by position, then by name.
            let mut repeating: Vec<_> = result
                .registrations
                .iter()
                .filter(|r| r.effect == *label && r.firing == Firing::Repeating)
                .collect();
            repeating.sort_by_key(|r| {
                (
                    r.span.map_or((u32::MAX, u32::MAX), |s| s.pos_key()),
                    r.display.clone(),
                )
            });
            let Some(first) = repeating.first() else {
                continue;
            };

            let mut names: Vec<&str> = repeating.iter().map(|r| r.display.as_str()).collect();
            names.dedup();
            let what = match names.as_slice() {
                [one] => format!("`{one}`"),
                many => many
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let mut d = Diagnostic::warn(
                Self::NAME,
                format!(
                    "this effect calls {what} but returns no cleanup — the registration is \
                     repeated every time the effect re-runs (and on every mount, twice under \
                     StrictMode) and nothing ever undoes it; return a function that tears it down"
                ),
            )
            .with_label(*label);
            if let Some(span) = first.span {
                d = d.with_range(span);
            }
            diags.push(d);
        }

        diags
    }
}
