use std::collections::HashMap;

use crate::domains::{AbstractDomain, StateValue};
use crate::ir::expr::SummaryValue;

// ── HookSummary trait ─────────────────────────────────────────────────────────

/// Abstract summary for a library hook without source.
/// Default `summarize` returns `Top`; override to be more precise.
pub trait HookSummary: Send + Sync {
    fn name(&self) -> &str;

    /// Compute the abstract return value of this hook given abstract arg values.
    /// Default: `Top` (most conservative completely unknown).
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::top()
    }

    /// Named members of the returned object that carry a contract of their own.
    ///
    /// This is the shape libraries actually publish: `useForm()` promises
    /// `setValue` is the same function at every render and promises nothing
    /// about `formState`. Everything not listed reads ⊤, so a member added to
    /// a library after this table was written is never credited with a
    /// stability nobody wrote down.
    ///
    /// Empty means the hook's return has no per-member contract, and
    /// [`Self::summarize`] alone answers for it.
    fn members(&self) -> &'static [(&'static str, SummaryValue)] {
        &[]
    }
}

// ── SummaryRegistry ───────────────────────────────────────────────────────────

/// Key: `(package, hook_name)`.  `package = None` for unscoped registrations
/// that match any import source (or hooks defined locally).
type SummaryKey = (Option<String>, String);

/// Registry mapping library hooks to their abstract summaries.
/// Lookup: `(package, name)` exact match first, then `(None, name)` fallback.
pub struct SummaryRegistry {
    summaries: HashMap<SummaryKey, Box<dyn HookSummary>>,
}

impl SummaryRegistry {
    pub fn new() -> Self {
        SummaryRegistry {
            summaries: HashMap::new(),
        }
    }

    /// Pre-populate with common TanStack Query, React Router and Next.js
    /// hooks, scoped to their respective NPM packages so they only match
    /// hooks that were actually imported from those packages.
    ///
    /// Registering a hook as ⊤ is not modelling it — it is recording that the
    /// hook is *known*, which is what separates a deliberate imprecision from
    /// the `analysis-limit/unknown-hook` Info that means "we could not even
    /// find this definition".
    pub fn new_with_common() -> Self {
        let mut r = Self::new();
        r.register_many_for_package("@tanstack/react-query", TANSTACK_HOOKS);
        r.register_many_for_package("react-router-dom", REACT_ROUTER_HOOKS);
        r.register_many_for_package("react-router", REACT_ROUTER_HOOKS);
        r.register_many_for_package("next/navigation", NEXT_NAVIGATION_HOOKS);
        r.register_many_for_package("next/router", &["useRouter"]);
        r.register_many_for_package("next/compat/router", &["useRouter"]);
        // The one Next hook whose *kind* is certain: App Router
        // `usePathname()` is typed `string`, and a primitive is compared by
        // value — so a `pathname` dep is never a per-render fresh reference.
        r.register_for_package("next/navigation", Box::new(StrTopSummary("usePathname")));

        // Per-member contracts (#94). Registering the *shape* is what lets a
        // destructured `const { setValue } = useForm()` resolve: the container
        // stays ⊤ and each named member answers for itself.
        for hook in ["useForm", "useFormContext"] {
            r.register_for_package(
                "react-hook-form",
                Box::new(ShapeSummary(hook, REACT_HOOK_FORM_MEMBERS)),
            );
        }
        for pkg in ["next/navigation", "next/router", "next/compat/router"] {
            r.register_for_package(
                pkg,
                Box::new(ShapeSummary("useRouter", NEXT_ROUTER_MEMBERS)),
            );
        }
        for (pkg, hook) in [("swr", "useSWR"), ("swr", "useSWRConfig")] {
            r.register_for_package(pkg, Box::new(ShapeSummary(hook, SWR_MEMBERS)));
        }
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

/// A hook whose return is an unknown **string**. Narrower than ⊤ in the one
/// way that matters downstream: a primitive is value-compared, so it can
/// never read as a fresh reference in a deps array.
struct StrTopSummary(&'static str);

impl HookSummary for StrTopSummary {
    fn name(&self) -> &str {
        self.0
    }
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::str_top()
    }
}

/// A hook whose return object has a published per-member contract.
///
/// The container itself stays ⊤: what these libraries document is that certain
/// *members* keep their identity, not that the object does.
struct ShapeSummary(&'static str, &'static [(&'static str, SummaryValue)]);

impl HookSummary for ShapeSummary {
    fn name(&self) -> &str {
        self.0
    }
    fn members(&self) -> &'static [(&'static str, SummaryValue)] {
        self.1
    }
}

/// react-hook-form's `useForm()` / `useFormContext()`.
///
/// The library documents these as stable for the lifetime of the form, which
/// is why its own docs show them omitted from deps arrays. Deliberately
/// absent: `formState` (a Proxy that changes as the form does), `watch`'s
/// *result*, and anything else — all ⊤.
const REACT_HOOK_FORM_MEMBERS: &[(&str, SummaryValue)] = &[
    ("register", SummaryValue::StableRef),
    ("unregister", SummaryValue::StableRef),
    ("setValue", SummaryValue::StableRef),
    ("getValues", SummaryValue::StableRef),
    ("getFieldState", SummaryValue::StableRef),
    ("setError", SummaryValue::StableRef),
    ("clearErrors", SummaryValue::StableRef),
    ("setFocus", SummaryValue::StableRef),
    ("resetField", SummaryValue::StableRef),
    ("reset", SummaryValue::StableRef),
    ("trigger", SummaryValue::StableRef),
    ("handleSubmit", SummaryValue::StableRef),
    ("watch", SummaryValue::StableRef),
    ("control", SummaryValue::StableRef),
];

/// Next.js App Router `useRouter()` — the router object and its methods are
/// documented stable, and apps omit them from deps for exactly that reason.
const NEXT_ROUTER_MEMBERS: &[(&str, SummaryValue)] = &[
    ("push", SummaryValue::StableRef),
    ("replace", SummaryValue::StableRef),
    ("refresh", SummaryValue::StableRef),
    ("prefetch", SummaryValue::StableRef),
    ("back", SummaryValue::StableRef),
    ("forward", SummaryValue::StableRef),
];

/// SWR. `mutate` is bound to the key and stable; `data`, `error` and
/// `isLoading` are the whole point of the hook changing, so they stay ⊤.
const SWR_MEMBERS: &[(&str, SummaryValue)] = &[("mutate", SummaryValue::StableRef)];

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

/// Next.js App Router hooks (`next/navigation`). All client-only; all opaque
/// to the engine except `usePathname`, refined above.
const NEXT_NAVIGATION_HOOKS: &[&str] = &[
    "useRouter",
    "usePathname",
    "useSearchParams",
    "useParams",
    "useSelectedLayoutSegment",
    "useSelectedLayoutSegments",
    "useServerInsertedHTML",
    "useLinkStatus",
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
                StateValue::null()
            }
        }
        let mut r = SummaryRegistry::new();
        r.register(Box::new(Fixed));
        // Unscoped entry matches any import source.
        assert!(r.contains("useFixed", None));
        assert!(r.contains("useFixed", Some("some-package")));
        let s = r.get("useFixed", None).unwrap();
        assert_eq!(s.summarize(&[]), StateValue::null());
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
                StateValue::null()
            }
        }
        struct Unscoped;
        impl HookSummary for Unscoped {
            fn name(&self) -> &str {
                "useX"
            }
            fn summarize(&self, _: &[StateValue]) -> StateValue {
                StateValue::top()
            }
        }
        let mut r = SummaryRegistry::new();
        r.register_for_package("pkg", Box::new(Scoped));
        r.register(Box::new(Unscoped));
        assert_eq!(
            r.get("useX", Some("pkg")).unwrap().summarize(&[]),
            StateValue::null()
        );
        assert_eq!(
            r.get("useX", None).unwrap().summarize(&[]),
            StateValue::top()
        );
    }

    #[test]
    fn default_summarize_returns_top() {
        let mut r = SummaryRegistry::new();
        r.register_many_for_package("my-pkg", &["useTopHook"]);
        let s = r.get("useTopHook", Some("my-pkg")).unwrap();
        assert_eq!(s.summarize(&[]), StateValue::top());
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
    fn common_next_hooks_registered() {
        let r = SummaryRegistry::new_with_common();
        for name in ["useRouter", "useSearchParams", "useParams", "usePathname"] {
            assert!(
                r.contains(name, Some("next/navigation")),
                "missing Next hook: {name}"
            );
        }
        assert!(r.contains("useRouter", Some("next/router")));
        // Package-scoped: a same-named hook from elsewhere must not match.
        assert!(!r.contains("useSearchParams", Some("my-own-lib")));
    }

    #[test]
    fn use_pathname_is_a_string_not_top() {
        let r = SummaryRegistry::new_with_common();
        let s = r.get("usePathname", Some("next/navigation")).unwrap();
        let v = s.summarize(&[]);
        assert_eq!(v, StateValue::str_top());
        assert!(!v.is_top_value(), "a string is narrower than ⊤");
    }

    #[test]
    fn use_context_stays_unknown_on_purpose() {
        // The engine has no model for it, and the `analysis-limit` Info that
        // says so is the largest signal in the corpora — registering a ⊤
        // summary would silence it (#28).
        let r = SummaryRegistry::new_with_common();
        assert!(!r.contains("useContext", Some("react")));
    }

    #[test]
    fn unknown_hook_not_in_common() {
        let r = SummaryRegistry::new_with_common();
        assert!(!r.contains("useMyCustomHook", None));
        assert!(!r.contains("useMyCustomHook", Some("@tanstack/react-query")));
    }
}
