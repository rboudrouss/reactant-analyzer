//! `server-component-hook` (ADR-026 §4): a hook called in a module Next
//! compiles into the server graph.

use std::fmt::Write as _;

use crate::ir::hooks::{HookEntry, HookProvenance};
use crate::project::nextjs::{USE_CLIENT, server_entry_kind, server_modules};
use crate::rules::{Diagnostic, Rule, RuleCtx, SafeCheck};

/// React's own hooks that only exist while rendering on the client. The
/// engine models the first five as dedicated `HookEntry` kinds; the rest
/// reach the IR as `Custom` rows with a `react` provenance.
const REACT_CLIENT_HOOKS: &[&str] = &[
    "useContext",
    "useId",
    "useTransition",
    "useDeferredValue",
    "useSyncExternalStore",
    "useImperativeHandle",
    "useDebugValue",
    "useOptimistic",
    "useActionState",
    "useFormStatus",
    "useEffectEvent",
];

/// Client-only hooks of the packages a Next app routes and submits through,
/// keyed by the import specifier they must have come from. Package-scoped on
/// purpose: a project's own `useRouter` proves nothing.
const PACKAGE_CLIENT_HOOKS: &[(&str, &[&str])] = &[
    (
        "next/navigation",
        &[
            "useRouter",
            "usePathname",
            "useSearchParams",
            "useParams",
            "useSelectedLayoutSegment",
            "useSelectedLayoutSegments",
            "useLinkStatus",
            "useServerInsertedHTML",
        ],
    ),
    ("next/router", &["useRouter"]),
    ("next/compat/router", &["useRouter"]),
    ("react-dom", &["useFormStatus", "useFormState"]),
];

/// Flags hooks called from a module Next.js compiles as a Server Component.
///
/// Server Components render once, on the server, with no state and no
/// commit phase — React has no hook to give them, and the render throws.
/// The finding is a `Warning` rather than an `Error` because the two facts it
/// rests on live outside the abstract domain: the App Router entry set comes
/// from a filename convention, and the server graph from import edges the
/// resolver may not have resolved.
///
/// Silent unless the program actually uses the RSC directive: without a
/// single `"use client"` anywhere, "this module is a Server Component" is not
/// a claim the file layout can support.
pub struct ServerComponentHook;

impl ServerComponentHook {
    pub(crate) const NAME: &'static str = "server-component-hook";
}

/// The client-only hook this entry calls, as it should be named in the
/// message; `None` when the entry may legally run on the server.
fn client_only_hook(entry: &HookEntry, prov: Option<&HookProvenance>) -> Option<String> {
    let named = |fallback: &str| {
        prov.map(|p| p.origin_hook.clone())
            .unwrap_or_else(|| fallback.to_string())
    };
    match entry {
        // Modelled React hooks: the entry kind *is* the proof, whichever
        // surface name produced it (`useReducer` files as `State`,
        // `useLayoutEffect` as `Effect`).
        HookEntry::State { .. } => Some(named("useState")),
        HookEntry::Effect { .. } => Some(named("useEffect")),
        HookEntry::Memo { .. } => Some(named("useMemo")),
        HookEntry::Callback { .. } => Some(named("useCallback")),
        HookEntry::Ref { .. } => Some(named("useRef")),
        // An opaque `useX` is not evidence on its own — plenty of `use`-named
        // helpers call no hook at all. Only the ones whose origin is a hook
        // documented as client-only count.
        HookEntry::Custom {
            name,
            import_source,
            ..
        } => {
            let origin = prov
                .map(|p| p.origin_hook.as_str())
                .unwrap_or(name.as_str());
            let is_react = prov.is_some_and(|p| p.react);
            let specifier = prov
                .and_then(|p| p.specifier.as_deref())
                .or(import_source.as_deref());
            let react_hit = is_react && REACT_CLIENT_HOOKS.contains(&origin);
            let package_hit = specifier.is_some_and(|s| {
                PACKAGE_CLIENT_HOOKS
                    .iter()
                    .any(|(pkg, hooks)| *pkg == s && hooks.contains(&origin))
            });
            (react_hit || package_hit).then(|| origin.to_string())
        }
        // A DOM handler in a server module is a different Next error
        // ("event handlers cannot be passed to Client Component props") and
        // not a hook call.
        HookEntry::Handler { .. } => None,
    }
}

impl Rule for ServerComponentHook {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    /// Applicable to the Server Components themselves — saying "verified" on
    /// a client component would claim a check that never had anything to do.
    fn safe_check(&self, ctx: &RuleCtx) -> Option<SafeCheck> {
        let table = &ctx.program().module_table;
        (table.any_declares(USE_CLIENT) && server_modules(table).contains(&ctx.comp().file))
            .then_some(SafeCheck {
                rule: Self::NAME,
                message: "this Server Component calls no client-only hook",
            })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let table = &ctx.program().module_table;
        if !table.any_declares(USE_CLIENT) {
            return vec![];
        }
        let comp = ctx.comp();
        if !server_modules(table).contains(&comp.file) {
            return vec![];
        }

        // Named hooks, first-seen order, deduped: several `useState` calls
        // are one missing directive, not several defects.
        let mut offenders: Vec<(String, &HookEntry)> = Vec::new();
        for entry in &comp.hooks {
            let prov = comp
                .hook_provenance
                .iter()
                .find(|p| p.label == entry.label());
            let Some(hook) = client_only_hook(entry, prov) else {
                continue;
            };
            if !offenders.iter().any(|(name, _)| *name == hook) {
                offenders.push((hook, entry));
            }
        }
        let Some((_, first)) = offenders.first() else {
            return vec![];
        };

        // One finding per component: the defect is the module's missing
        // directive, and it is fixed once however many hooks it holds.
        let named: Vec<&str> = offenders
            .iter()
            .take(3)
            .map(|(name, _)| name.as_str())
            .collect();
        let mut list = named
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ");
        if offenders.len() > named.len() {
            let _ = write!(list, " and {} more", offenders.len() - named.len());
        }
        let verb = if offenders.len() == 1 { "is" } else { "are" };
        let why = match server_entry_kind(&comp.file) {
            Some(kind) => format!("this file is an App Router `{kind}`"),
            None => "this module is imported into the App Router's server graph".to_string(),
        };

        let mut d = Diagnostic::warn(
            Self::NAME,
            format!(
                "{list} {verb} called in a Server Component — {why} and no `\"use client\"` \
                 directive covers it, so React renders it on the server, where hooks do not \
                 exist; add `\"use client\"` at the top of the file, or move the stateful \
                 part into a child component that declares it"
            ),
        )
        .with_label(first.label());
        if let Some(span) = first.span() {
            d = d.with_range(span);
        }
        vec![d]
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Expr, Prim};

    fn prov(origin: &str, react: bool, specifier: Option<&str>) -> HookProvenance {
        HookProvenance {
            label: 0,
            origin_hook: origin.to_string(),
            react,
            specifier: specifier.map(str::to_string),
            file: None,
            inlined: false,
            span: None,
        }
    }

    fn custom(name: &str, import_source: Option<&str>) -> HookEntry {
        HookEntry::Custom {
            label: 0,
            name: name.to_string(),
            args: vec![],
            deps: None,
            binding: None,
            import_source: import_source.map(str::to_string),
            resolved_file: None,
            span: None,
        }
    }

    #[test]
    fn a_modelled_hook_is_named_by_its_origin() {
        let state = HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Unit),
            span: None,
        };
        // `useReducer` files as a `State` entry; the message says so.
        assert_eq!(
            client_only_hook(&state, Some(&prov("useReducer", true, Some("react")))),
            Some("useReducer".to_string())
        );
        // No provenance row: still evidence, named by the entry kind.
        assert_eq!(client_only_hook(&state, None), Some("useState".to_string()));
    }

    #[test]
    fn a_documented_client_only_hook_counts() {
        let entry = custom("usePathname", Some("next/navigation"));
        assert_eq!(
            client_only_hook(
                &entry,
                Some(&prov("usePathname", false, Some("next/navigation")))
            ),
            Some("usePathname".to_string())
        );
        let ctx = custom("useContext", Some("react"));
        assert_eq!(
            client_only_hook(&ctx, Some(&prov("useContext", true, Some("react")))),
            Some("useContext".to_string())
        );
    }

    #[test]
    fn an_opaque_use_named_call_is_not_evidence() {
        // The conservative half: plenty of `use`-named helpers call no hook,
        // and a third-party one we have no fact about could be either.
        let entry = custom("useSession", Some("next-auth/react"));
        assert_eq!(
            client_only_hook(
                &entry,
                Some(&prov("useSession", false, Some("next-auth/react")))
            ),
            None
        );
        let local = custom("useSlug", None);
        assert_eq!(
            client_only_hook(&local, Some(&prov("useSlug", false, None))),
            None
        );
    }

    #[test]
    fn the_package_scope_is_load_bearing() {
        // A project's own `useRouter` is not `next/navigation`'s.
        let mine = custom("useRouter", Some("@/hooks/router"));
        assert_eq!(
            client_only_hook(
                &mine,
                Some(&prov("useRouter", false, Some("@/hooks/router")))
            ),
            None
        );
    }

    #[test]
    fn a_dom_handler_is_a_different_error() {
        let handler = HookEntry::Handler {
            label: 0,
            event: "click".to_string(),
            body_cfg: crate::ir::CFG {
                entry: 0,
                blocks: Default::default(),
                edges: vec![],
            },
            span: None,
        };
        assert_eq!(client_only_hook(&handler, None), None);
    }
}
