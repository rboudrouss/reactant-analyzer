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

/// Key: `(package, hook_name)`.  `package = None` for unscoped registrations
/// that match any import source (or hooks defined locally).
type SummaryKey = (Option<String>, String);

/// Registry mapping library hooks to their abstract summaries.
///
/// Lookup is import-source-aware: a hook imported from `@tanstack/react-query`
/// only matches summaries registered for that exact package, preventing
/// accidental shadowing when a user defines their own `useQuery`.
///
/// # Lookup order
///
/// 1. `(Some(import_source), name)` — exact package match.
/// 2. `(None, name)` — unscoped fallback (matches any import source or none).
pub struct SummaryRegistry {
    summaries: HashMap<SummaryKey, Box<dyn HookSummary>>,
}

impl SummaryRegistry {
    pub fn new() -> Self {
        SummaryRegistry {
            summaries: HashMap::new(),
        }
    }

    /// Pre-populate with common TanStack Query and React Router hooks,
    /// scoped to their respective NPM packages so they only match hooks
    /// that were actually imported from those packages.
    pub fn new_with_common() -> Self {
        let mut r = Self::new();
        r.register_many_for_package("@tanstack/react-query", TANSTACK_HOOKS);
        r.register_many_for_package("react-router-dom", REACT_ROUTER_HOOKS);
        r.register_many_for_package("react-router", REACT_ROUTER_HOOKS);
        r
    }

    /// Register a hook summary scoped to a specific NPM package.
    /// Only matches hooks whose import source equals `package`.
    pub fn register_for_package(&mut self, package: impl Into<String>, s: Box<dyn HookSummary>) {
        let key = (Some(package.into()), s.name().to_string());
        self.summaries.insert(key, s);
    }

    /// Register an unscoped hook summary that matches any import source (or none).
    /// Use for hooks defined locally or when the source package is unknown.
    pub fn register(&mut self, s: Box<dyn HookSummary>) {
        let key = (None, s.name().to_string());
        self.summaries.insert(key, s);
    }

    /// Look up a summary for `name` imported from `import_source`.
    /// Tries the scoped entry first, then the unscoped fallback.
    pub fn get(&self, name: &str, import_source: Option<&str>) -> Option<&dyn HookSummary> {
        if let Some(src) = import_source {
            let scoped: SummaryKey = (Some(src.to_string()), name.to_string());
            if let Some(s) = self.summaries.get(&scoped) {
                return Some(s.as_ref());
            }
        }
        let unscoped: SummaryKey = (None, name.to_string());
        self.summaries.get(&unscoped).map(|s| s.as_ref())
    }

    /// Returns `true` if a summary exists for `name` / `import_source` (same lookup order as `get`).
    pub fn contains(&self, name: &str, import_source: Option<&str>) -> bool {
        self.get(name, import_source).is_some()
    }

    fn register_many_for_package(&mut self, package: &'static str, names: &[&'static str]) {
        for &name in names {
            self.summaries.insert(
                (Some(package.to_string()), name.to_string()),
                Box::new(TopSummary(name)),
            );
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
        assert!(!r.contains("useQuery", None));
        assert!(r.get("useQuery", None).is_none());
    }

    #[test]
    fn register_unscoped_and_get() {
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
        // Unscoped entry matches any import source.
        assert!(r.contains("useFixed", None));
        assert!(r.contains("useFixed", Some("some-package")));
        let s = r.get("useFixed", None).unwrap();
        assert_eq!(s.summarize(&[]), StateValue::Null);
    }

    #[test]
    fn register_for_package_only_matches_that_package() {
        struct Fixed;
        impl HookSummary for Fixed {
            fn name(&self) -> &str {
                "useData"
            }
        }
        let mut r = SummaryRegistry::new();
        r.register_for_package("my-lib", Box::new(Fixed));
        assert!(r.contains("useData", Some("my-lib")));
        assert!(!r.contains("useData", Some("other-lib")));
        assert!(!r.contains("useData", None));
    }

    #[test]
    fn scoped_takes_priority_over_unscoped() {
        struct Scoped;
        impl HookSummary for Scoped {
            fn name(&self) -> &str {
                "useX"
            }
            fn summarize(&self, _: &[StateValue]) -> StateValue {
                StateValue::Null
            }
        }
        struct Unscoped;
        impl HookSummary for Unscoped {
            fn name(&self) -> &str {
                "useX"
            }
            fn summarize(&self, _: &[StateValue]) -> StateValue {
                StateValue::Top
            }
        }
        let mut r = SummaryRegistry::new();
        r.register_for_package("pkg", Box::new(Scoped));
        r.register(Box::new(Unscoped));
        assert_eq!(
            r.get("useX", Some("pkg")).unwrap().summarize(&[]),
            StateValue::Null
        );
        assert_eq!(r.get("useX", None).unwrap().summarize(&[]), StateValue::Top);
    }

    #[test]
    fn default_summarize_returns_top() {
        let mut r = SummaryRegistry::new();
        r.register_many_for_package("my-pkg", &["useTopHook"]);
        let s = r.get("useTopHook", Some("my-pkg")).unwrap();
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
            assert!(
                r.contains(name, Some("@tanstack/react-query")),
                "missing TanStack hook: {name}"
            );
            // Must NOT match without correct package.
            assert!(
                !r.contains(name, Some("something-else")),
                "should not match wrong package: {name}"
            );
        }
    }

    #[test]
    fn common_router_hooks_registered() {
        let r = SummaryRegistry::new_with_common();
        for name in ["useNavigate", "useParams", "useLocation", "useSearchParams"] {
            assert!(
                r.contains(name, Some("react-router-dom"))
                    || r.contains(name, Some("react-router")),
                "missing React Router hook: {name}"
            );
        }
    }

    #[test]
    fn unknown_hook_not_in_common() {
        let r = SummaryRegistry::new_with_common();
        assert!(!r.contains("useMyCustomHook", None));
        assert!(!r.contains("useMyCustomHook", Some("@tanstack/react-query")));
    }
}
