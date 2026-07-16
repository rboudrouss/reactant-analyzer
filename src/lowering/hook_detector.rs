use oxc_ast::ast::*;

/// A user-defined custom hook function detected in the AST.
#[derive(Debug)]
pub struct HookCandidate<'a> {
    pub name: String,
    pub params: &'a FormalParameters<'a>,
    pub body: &'a FunctionBody<'a>,
}

/// Detect all user-defined custom hook functions (`use*`) in `program`.
/// Excludes React built-in hooks.
pub fn detect_custom_hooks<'a>(program: &'a Program<'a>) -> Vec<HookCandidate<'a>> {
    let mut out = Vec::new();
    for stmt in &program.body {
        collect_from_stmt(stmt, &mut out);
    }
    out
}

// ── Top-level statement dispatch ──────────────────────────────────────────────

fn collect_from_stmt<'a>(stmt: &'a Statement<'a>, out: &mut Vec<HookCandidate<'a>>) {
    match stmt {
        Statement::FunctionDeclaration(func) => try_add_fn(func, None, out),
        Statement::VariableDeclaration(decl) => {
            for vd in &decl.declarations {
                try_add_var_decl(vd, out);
            }
        }
        Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                try_add_fn(func, None, out);
            }
            ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) => {
                // Anonymous default export of a hook is unusual but handle gracefully.
                if let Some(name) = extract_arrow_hook_name(arrow) {
                    try_add_arrow_with_name(&name, arrow, out);
                }
            }
            _ => {}
        },
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(decl) = &exp.declaration {
                collect_from_decl(decl, out);
            }
        }
        _ => {}
    }
}

fn collect_from_decl<'a>(decl: &'a Declaration<'a>, out: &mut Vec<HookCandidate<'a>>) {
    match decl {
        Declaration::FunctionDeclaration(func) => try_add_fn(func, None, out),
        Declaration::VariableDeclaration(vd) => {
            for vd in &vd.declarations {
                try_add_var_decl(vd, out);
            }
        }
        _ => {}
    }
}

// ── Candidate construction ─────────────────────────────────────────────────────

fn try_add_fn<'a>(
    func: &'a Function<'a>,
    name_override: Option<&str>,
    out: &mut Vec<HookCandidate<'a>>,
) {
    let name =
        name_override.unwrap_or_else(|| func.id.as_ref().map(|id| id.name.as_str()).unwrap_or(""));
    if !is_custom_hook(name) {
        return;
    }
    let Some(body) = func.body.as_deref() else {
        return;
    };
    out.push(HookCandidate {
        name: name.to_owned(),
        params: &func.params,
        body,
    });
}

fn try_add_var_decl<'a>(vd: &'a VariableDeclarator<'a>, out: &mut Vec<HookCandidate<'a>>) {
    let name = match &vd.id {
        BindingPattern::BindingIdentifier(id) => id.name.as_str(),
        _ => return,
    };
    if !is_custom_hook(name) {
        return;
    }
    let Some(init) = &vd.init else { return };
    match init {
        Expression::ArrowFunctionExpression(arrow) => {
            try_add_arrow_with_name(name, arrow, out);
        }
        Expression::FunctionExpression(func) => {
            let Some(body) = func.body.as_deref() else {
                return;
            };
            out.push(HookCandidate {
                name: name.to_owned(),
                params: &func.params,
                body,
            });
        }
        _ => {}
    }
}

fn try_add_arrow_with_name<'a>(
    name: &str,
    arrow: &'a ArrowFunctionExpression<'a>,
    out: &mut Vec<HookCandidate<'a>>,
) {
    out.push(HookCandidate {
        name: name.to_owned(),
        params: &arrow.params,
        body: &arrow.body,
    });
}

fn extract_arrow_hook_name(arrow: &ArrowFunctionExpression) -> Option<String> {
    // Anonymous arrow exports rarely have hook names; return None.
    let _ = arrow;
    None
}

// ── Detection rules ────────────────────────────────────────────────────────────

/// Returns `true` iff `name` is a user-defined custom hook.
/// Rule: starts with `use`, length > 3, not a React built-in hook.
/// Any locally-defined `use*` function is a custom hook — INCLUDING one named
/// like a React built-in (`function useMemo(name, options)`, memos): JS
/// scoping makes the local definition shadow the React import/global, and the
/// call-site classification (`ImportCtx::callee_is_react`) relies on these
/// names to resolve the collision.
fn is_custom_hook(name: &str) -> bool {
    name.starts_with("use") && name.len() > 3
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
