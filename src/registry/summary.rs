use std::collections::HashMap;

use crate::domains::StateValue;

// ── HookSummary trait ─────────────────────────────────────────────────────────

/// Abstract summary for a library hook that has no source available.
///
/// Implementors describe what a hook returns (conservatively) so the fixpoint
/// can provide a more precise binding than opaque `Unit`.  The default
/// implementation returns `Top` — sound but imprecise.
///
/// # Adding a new library hook
///
/// ```rust
/// use reactant::registry::summary::{HookSummary, SummaryRegistry};
/// use reactant::domains::{StateValue, Stability};
///
/// struct UseMyHook;
/// impl HookSummary for UseMyHook {
///     fn name(&self) -> &str { "useMyHook" }
///     fn summarize(&self, _args: &[StateValue]) -> StateValue {
///         StateValue::Reference(Stability::Stable)
///     }
/// }
///
/// let mut reg = SummaryRegistry::new();
/// reg.register(Box::new(UseMyHook));
/// ```
pub trait HookSummary: Send + Sync {
    fn name(&self) -> &str;

    /// Compute the abstract return value of this hook given abstract arg values.
    /// Default: `Top` (most conservative — completely unknown).
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::Top
    }
}

// ── SummaryRegistry ───────────────────────────────────────────────────────────

pub struct SummaryRegistry {
    summaries: HashMap<String, Box<dyn HookSummary>>,
}

impl SummaryRegistry {
    pub fn new() -> Self {
        SummaryRegistry {
            summaries: HashMap::new(),
        }
    }

    /// Pre-populate with common TanStack Query and React Router hooks.
    pub fn new_with_common() -> Self {
        let mut r = Self::new();
        r.register_many(TANSTACK_HOOKS);
        r.register_many(REACT_ROUTER_HOOKS);
        r
    }

    pub fn register(&mut self, s: Box<dyn HookSummary>) {
        self.summaries.insert(s.name().to_string(), s);
    }

    pub fn get(&self, name: &str) -> Option<&dyn HookSummary> {
        self.summaries.get(name).map(|s| s.as_ref())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.summaries.contains_key(name)
    }

    fn register_many(&mut self, names: &[&'static str]) {
        for &name in names {
            self.summaries
                .insert(name.to_string(), Box::new(TopSummary(name)));
        }
    }
}

impl Default for SummaryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Blanket Top-returning summary ─────────────────────────────────────────────

struct TopSummary(&'static str);

impl HookSummary for TopSummary {
    fn name(&self) -> &str {
        self.0
    }
    // summarize returns Top via default
}

// ── Known library hook lists ──────────────────────────────────────────────────

const TANSTACK_HOOKS: &[&str] = &[
    "useQuery",
    "useMutation",
    "useInfiniteQuery",
    "useSuspenseQuery",
    "useSuspenseInfiniteQuery",
    "useQueries",
    "useSuspenseQueries",
    "useQueryClient",
    "useIsFetching",
    "useIsMutating",
    "usePrefetchQuery",
    "usePrefetchInfiniteQuery",
    "useQueryErrorResetBoundary",
];

const REACT_ROUTER_HOOKS: &[&str] = &[
    "useNavigate",
    "useParams",
    "useLocation",
    "useSearchParams",
    "useMatch",
    "useRouteError",
    "useActionData",
    "useLoaderData",
    "useNavigation",
    "useResolvedPath",
    "useHref",
    "useOutlet",
    "useOutletContext",
    "useRoutes",
    "useBlocker",
    "useFetcher",
    "useFetchers",
    "useFormAction",
    "useSubmit",
    "useRevalidator",
    "useRouteLoaderData",
    "useMatches",
    "useNavigationType",
    "useBeforeUnload",
];

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_contains_nothing() {
        let r = SummaryRegistry::new();
        assert!(!r.contains("useQuery"));
        assert!(r.get("useQuery").is_none());
    }

    #[test]
    fn register_and_get() {
        struct Fixed;
        impl HookSummary for Fixed {
            fn name(&self) -> &str {
                "useFixed"
            }
            fn summarize(&self, _: &[StateValue]) -> StateValue {
                StateValue::Null
            }
        }
        let mut r = SummaryRegistry::new();
        r.register(Box::new(Fixed));
        assert!(r.contains("useFixed"));
        let s = r.get("useFixed").unwrap();
        assert_eq!(s.summarize(&[]), StateValue::Null);
    }

    #[test]
    fn default_summarize_returns_top() {
        let mut r = SummaryRegistry::new();
        r.register_many(&["useTopHook"]);
        let s = r.get("useTopHook").unwrap();
        assert_eq!(s.summarize(&[]), StateValue::Top);
    }

    #[test]
    fn common_tanstack_hooks_registered() {
        let r = SummaryRegistry::new_with_common();
        for name in [
            "useQuery",
            "useMutation",
            "useInfiniteQuery",
            "useQueryClient",
        ] {
            assert!(r.contains(name), "missing TanStack hook: {name}");
        }
    }

    #[test]
    fn common_router_hooks_registered() {
        let r = SummaryRegistry::new_with_common();
        for name in ["useNavigate", "useParams", "useLocation", "useSearchParams"] {
            assert!(r.contains(name), "missing React Router hook: {name}");
        }
    }

    #[test]
    fn unknown_hook_not_in_common() {
        let r = SummaryRegistry::new_with_common();
        assert!(!r.contains("useMyCustomHook"));
    }
}
