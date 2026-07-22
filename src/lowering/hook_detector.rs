use oxc_ast::ast::*;

use super::Candidate;
use super::detector::{self, Classify, FnItem};

/// A user-defined custom hook function detected in the AST.
pub type HookCandidate<'a> = Candidate<'a>;

/// Detect all user-defined custom hook functions (`use*`) in `program`.
/// Excludes React built-in hooks.
pub fn detect_custom_hooks<'a>(program: &'a Program<'a>) -> Vec<HookCandidate<'a>> {
    detector::detect_fns(program, classify, Some(default))
}

/// A function is a custom hook iff its name follows the hook convention. The
/// body is irrelevant (a hook may or may not return JSX).
fn classify(item: &FnItem) -> bool {
    is_custom_hook(item.name)
}

/// `export default function useThing()`. Only a *named* function declaration
/// qualifies: an anonymous arrow default export carries no hook name to match
/// against, so it is not a hook (there is nothing to classify).
fn default<'a>(
    exp: &'a ExportDefaultDeclaration<'a>,
    classify: Classify,
    out: &mut Vec<Candidate<'a>>,
) {
    if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &exp.declaration {
        detector::consider_fn(func, None, None, classify, out);
    }
}

// ── Detection rules ────────────────────────────────────────────────────────────

/// Returns `true` iff `name` is a user-defined custom hook.
/// Rule: React's hook-name convention (`use` + uppercase/digit), shared with
/// the utility detector via [`super::is_hook_name`] so the two never diverge
/// (a lowercase-4th-char name like `useful` is a utility, not a hook, and must
/// not be classified as both).
/// Any locally-defined hook-named function is a custom hook — INCLUDING one
/// named like a React built-in (`function useMemo(name, options)`, memos): JS
/// scoping makes the local definition shadow the React import/global, and the
/// call-site classification (`ImportCtx::callee_is_react`) relies on these
/// names to resolve the collision.
fn is_custom_hook(name: &str) -> bool {
    super::is_hook_name(name)
}

#[cfg(test)]
const BUILTIN_HOOKS: &[&str] = &[
    "useState",
    "useEffect",
    "useMemo",
    "useCallback",
    "useRef",
    "useReducer",
    "useContext",
    "useLayoutEffect",
    "useInsertionEffect",
    "useId",
    "useTransition",
    "useDeferredValue",
    "useImperativeHandle",
    "useDebugValue",
    "useSyncExternalStore",
];

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn hook_names(src: &str) -> Vec<String> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        detect_custom_hooks(&ret.program)
            .into_iter()
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn fn_declaration_detected() {
        assert_eq!(
            hook_names("function useCounter(initial) { return 0; }"),
            vec!["useCounter"]
        );
    }

    #[test]
    fn arrow_const_detected() {
        assert_eq!(
            hook_names("const useData = (id) => { return null; };"),
            vec!["useData"]
        );
    }

    #[test]
    fn export_named_detected() {
        assert_eq!(
            hook_names("export function useAsync(fn) { return null; }"),
            vec!["useAsync"]
        );
    }

    #[test]
    fn component_not_detected() {
        assert!(hook_names("function Counter() { return <div/>; }").is_empty());
    }

    #[test]
    fn lowercase_fourth_char_is_not_a_hook() {
        // `useful`/`userId` are NOT hooks by React's rule (a lowercase 4th char).
        // They used to be caught by the loose `len > 3` predicate and end up
        // classified as BOTH a hook and a utility. Now they are utility-only.
        assert!(hook_names("function useful(x) { return x; }").is_empty());
        assert!(hook_names("function userId(u) { return u.id; }").is_empty());
        // Genuine hook names (uppercase or digit after `use`) still detected.
        assert_eq!(
            hook_names("function use2FA() { return null; }"),
            vec!["use2FA"]
        );
    }

    #[test]
    fn shadowing_builtin_names_detected() {
        // A local definition shadows the React import/global (JS scoping);
        // it must be a candidate so call sites resolve to it, not to React.
        for builtin in BUILTIN_HOOKS {
            let src = format!("function {builtin}() {{}}");
            let names = hook_names(&src);
            assert_eq!(
                names,
                vec![builtin.to_string()],
                "local {builtin} must be detected as a custom hook"
            );
        }
    }

    #[test]
    fn too_short_excluded() {
        // "use" (len 3) is not a custom hook
        assert!(hook_names("function use() { return null; }").is_empty());
    }

    #[test]
    fn non_use_prefix_excluded() {
        assert!(hook_names("function helper() { return null; }").is_empty());
    }
}
