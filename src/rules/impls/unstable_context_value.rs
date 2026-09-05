use crate::rules::helpers::providers::{ValueIdentity, collect_provider_sites};
use crate::rules::{Diagnostic, Rule, RuleCtx};

/// Fires when a context provider hands its consumers a brand-new object on
/// every render.
///
/// `useContext` re-renders a consumer whenever the provided value changes
/// identity, and React compares with `Object.is`. A value allocated in the
/// provider's own render body — `value={{ a, b }}`, or a `const v = { a, b }`
/// that is never memoized — is a different object every time, so *every*
/// consumer re-renders whenever the providing component does, however deep the
/// tree and however little actually changed. The fix is `useMemo` on the value
/// (or lifting it out of render when it is constant).
///
/// This is the class eslint cannot reach: the inline-literal form is visible
/// syntactically, but a value bound through locals, a branch, or a helper is
/// only fresh-or-not once the abstract value is known.
///
/// Two bounds:
///
/// - **A proven context only.** `<X.Provider>` is a provider of *this* rule's
///   kind only when `X` is a module-level `createContext(…)` reached through a
///   React import. An imported context is not proven here and is skipped —
///   precision lost, never soundness.
/// - **A proven-fresh value only.** `ValueIdentity::FreshEveryRender` is a
///   must-fact (`is_unstable_reference_only`, not the `stability` verdict whose
///   `per-render` also covers a moving primitive — a number that changes is not
///   an identity problem).
///
/// Warning, never Error: the fresh reference is certain, but what it costs is
/// not — a provider with two consumers that re-render anyway pays nothing.
pub struct UnstableContextValue;

impl UnstableContextValue {
    pub(crate) const NAME: &'static str = "unstable-context-value";
}

impl Rule for UnstableContextValue {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        (!collect_provider_sites(ctx.comp()).is_empty()).then_some(crate::rules::SafeCheck {
            rule: Self::NAME,
            message: "every context value this component provides keeps its identity across renders",
        })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        collect_provider_sites(ctx.comp())
            .into_iter()
            .filter(|site| site.identity == ValueIdentity::FreshEveryRender)
            .map(|site| {
                let context = site.context;
                let mut d = Diagnostic::warn(
                    Self::NAME,
                    format!(
                        "`{context}.Provider` is given a newly allocated value on every render. \
                         `Object.is` fails for every consumer, so each `useContext({context})` \
                         re-renders whenever this component does, even when nothing in the value \
                         changed; wrap the value in `useMemo`"
                    ),
                );
                if let Some(span) = site.span {
                    d = d.with_range(span);
                }
                d
            })
            .collect()
    }
}
