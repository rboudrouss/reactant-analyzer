use std::collections::HashMap;

use crate::ir::{
    cfg::{BasicBlock, CFG, Terminator},
    expr::{Expr, Prim},
    hooks::HookEntry,
    stmt::Stmt,
    types::{BlockId, HookLabel},
};

/// Walk `cfg` in block-id order, extract all top-level hook calls into a
/// `Vec<HookEntry>`, and rewrite the affected statements in-place.
///
/// Labels are assigned in ascending block-id + statement order, which matches
/// textual source order for ~95% of React code.
///
/// Destructuring of useState/useReducer is resolved:
///   `__arr_N[0]` → `StateVal(L)`, `__arr_N[1]` → `StateSetter(L)`
pub fn extract_hooks(cfg: &mut CFG) -> Vec<HookEntry> {
    let mut label: HookLabel = 0;
    let mut hooks: Vec<HookEntry> = Vec::new();
    // Maps array-destructuring temps (e.g. "__arr_42") → hook label, for useState/useReducer.
    let mut state_temps: HashMap<String, HookLabel> = HashMap::new();

    let mut ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();
    ids.sort_unstable();

    for id in ids {
        let old = std::mem::take(&mut cfg.blocks.get_mut(&id).unwrap().stmts);
        let mut new: Vec<Stmt> = Vec::with_capacity(old.len());

        for stmt in old {
            process_stmt(stmt, &mut new, &mut hooks, &mut label, &mut state_temps);
        }

        cfg.blocks.get_mut(&id).unwrap().stmts = new;
    }

    hooks
}

fn process_stmt(
    stmt: Stmt,
    out: &mut Vec<Stmt>,
    hooks: &mut Vec<HookEntry>,
    label: &mut HookLabel,
    state_temps: &mut HashMap<String, HookLabel>,
) {
    match stmt {
        Stmt::Let { var, rhs } => match try_consume_hook_call(rhs) {
            Ok((name, args)) => {
                let lbl = *label;
                *label += 1;
                let is_state_like = matches!(name.as_str(), "useState" | "useReducer");
                let is_arr_temp = var.starts_with("__arr_");

                if let Some(entry) = make_hook_entry(&name, lbl, args) {
                    hooks.push(entry);
                }

                if is_state_like && is_arr_temp {
                    // Array-destructured useState/useReducer: drop the temp Let,
                    // subsequent IndexAccess stmts will be rewritten by rewrite_expr.
                    state_temps.insert(var, lbl);
                } else {
                    out.push(Stmt::Let { var, rhs: hook_result_expr(&name, lbl) });
                }
            }
            Err(rhs) => {
                out.push(Stmt::Let { var, rhs: rewrite_expr(rhs, state_temps) });
            }
        },
        Stmt::ExprStmt(expr) => match try_consume_hook_call(expr) {
            Ok((name, args)) => {
                let lbl = *label;
                *label += 1;
                if let Some(entry) = make_hook_entry(&name, lbl, args) {
                    hooks.push(entry);
                }
                // useEffect and similar void hooks: no stmt emitted.
            }
            Err(expr) => {
                out.push(Stmt::ExprStmt(rewrite_expr(expr, state_temps)));
            }
        },
        Stmt::Assign { var, rhs } => {
            out.push(Stmt::Assign { var, rhs: rewrite_expr(rhs, state_temps) });
        }
    }
}

// ── Hook call detection ───────────────────────────────────────────────────────

/// Returns `Ok((hook_name, args))` if `expr` is a `use*` call; else `Err(expr)`.
fn try_consume_hook_call(expr: Expr) -> Result<(String, Vec<Expr>), Expr> {
    match expr {
        Expr::Call { fn_, args } => match hook_name_from_callee(&fn_) {
            Some(name) => Ok((name, args)),
            None => Err(Expr::Call { fn_, args }),
        },
        other => Err(other),
    }
}

fn hook_name_from_callee(fn_: &Expr) -> Option<String> {
    match fn_ {
        Expr::Var(name) if name.starts_with("use") && name.len() > 3 => Some(name.clone()),
        // React.useState / React.useEffect / etc.
        Expr::FieldAccess { field, .. } if field.starts_with("use") && field.len() > 3 => {
            Some(field.clone())
        }
        _ => None,
    }
}

// ── HookEntry construction ────────────────────────────────────────────────────

fn make_hook_entry(name: &str, label: HookLabel, args: Vec<Expr>) -> Option<HookEntry> {
    let mut it = args.into_iter();
    match name {
        "useState" => {
            let init = it.next().unwrap_or(Expr::Lit(Prim::Unit));
            Some(HookEntry::State { label, init })
        }
        "useEffect" => {
            let body_cfg = it.next().and_then(expr_into_cfg).unwrap_or_else(fallback_cfg);
            let deps = it.next().and_then(expr_into_deps);
            Some(HookEntry::Effect { label, body_cfg, deps })
        }
        "useMemo" => {
            let body_cfg = it.next().and_then(expr_into_cfg).unwrap_or_else(fallback_cfg);
            let deps = it.next().and_then(expr_into_deps).unwrap_or_default();
            Some(HookEntry::Memo { label, body_cfg, deps })
        }
        "useCallback" => {
            let body_cfg = it.next().and_then(expr_into_cfg).unwrap_or_else(fallback_cfg);
            let deps = it.next().and_then(expr_into_deps).unwrap_or_default();
            Some(HookEntry::Callback { label, body_cfg, deps })
        }
        "useRef" => {
            let init = it.next().unwrap_or(Expr::Lit(Prim::Null));
            Some(HookEntry::Ref { label, init })
        }
        _ if name.starts_with("use") => {
            let args: Vec<Expr> = it.collect();
            Some(HookEntry::Custom { label, name: name.to_string(), args, deps: None })
        }
        _ => None,
    }
}

/// IR expression that replaces a hook call at its binding site.
fn hook_result_expr(name: &str, label: HookLabel) -> Expr {
    match name {
        "useState" | "useReducer" => Expr::StateVal(label),
        "useMemo" => Expr::MemoVal(label),
        "useCallback" => Expr::CallbackVal(label),
        _ => Expr::Lit(Prim::Unit),
    }
}

// ── Argument extraction ───────────────────────────────────────────────────────

fn expr_into_cfg(expr: Expr) -> Option<CFG> {
    match expr {
        Expr::FnLit { body_cfg, .. } => Some(*body_cfg),
        _ => None,
    }
}

fn expr_into_deps(expr: Expr) -> Option<Vec<Expr>> {
    match expr {
        Expr::ArrayLit(elems) => Some(elems),
        _ => None,
    }
}

fn fallback_cfg() -> CFG {
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(0, BasicBlock { id: 0, stmts: vec![], term: Terminator::Unreachable });
    CFG { entry: 0, blocks, edges: vec![] }
}

// ── Expr rewriting ────────────────────────────────────────────────────────────

/// Recursively rewrite `expr`, substituting array-index accesses into state temps:
///   `state_temp[0]` → `StateVal(L)`, `state_temp[1]` → `StateSetter(L)`
fn rewrite_expr(expr: Expr, state_temps: &HashMap<String, HookLabel>) -> Expr {
    match expr {
        Expr::IndexAccess { arr, idx } => match (*arr, *idx) {
            (Expr::Var(v), Expr::Lit(Prim::Int(i))) if state_temps.contains_key(&v) => {
                let &lbl = state_temps.get(&v).unwrap();
                match i {
                    0 => Expr::StateVal(lbl),
                    1 => Expr::StateSetter(lbl),
                    _ => Expr::Lit(Prim::Unit),
                }
            }
            (arr, idx) => Expr::IndexAccess {
                arr: Box::new(rewrite_expr(arr, state_temps)),
                idx: Box::new(rewrite_expr(idx, state_temps)),
            },
        },
        Expr::FieldAccess { obj, field } => Expr::FieldAccess {
            obj: Box::new(rewrite_expr(*obj, state_temps)),
            field,
        },
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(rewrite_expr(*lhs, state_temps)),
            rhs: Box::new(rewrite_expr(*rhs, state_temps)),
        },
        Expr::UnaryOp { op, arg } => {
            Expr::UnaryOp { op, arg: Box::new(rewrite_expr(*arg, state_temps)) }
        }
        Expr::Call { fn_, args } => Expr::Call {
            fn_: Box::new(rewrite_expr(*fn_, state_temps)),
            args: args.into_iter().map(|a| rewrite_expr(a, state_temps)).collect(),
        },
        Expr::ArrayLit(elems) => {
            Expr::ArrayLit(elems.into_iter().map(|e| rewrite_expr(e, state_temps)).collect())
        }
        Expr::ObjectLit(props) => Expr::ObjectLit(
            props.into_iter().map(|(k, v)| (k, rewrite_expr(v, state_temps))).collect(),
        ),
        Expr::TSAnnotated(inner, ty) => {
            Expr::TSAnnotated(Box::new(rewrite_expr(*inner, state_temps)), ty)
        }
        other => other,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{expr::Prim, hooks::HookEntry, stmt::Stmt};
    use crate::lowering::cfg_builder::build_cfg;
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn parse_and_extract(src: &str) -> (CFG, Vec<HookEntry>) {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        let mut cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| build_cfg(b)),
                _ => None,
            })
            .expect("no function found");
        let hooks = extract_hooks(&mut cfg);
        (cfg, hooks)
    }

    fn entry_stmts(cfg: &CFG) -> &[Stmt] {
        &cfg.blocks[&cfg.entry].stmts
    }

    fn find_let_rhs<'a>(stmts: &'a [Stmt], name: &str) -> Option<&'a Expr> {
        stmts.iter().find_map(|s| match s {
            Stmt::Let { var, rhs } if var == name => Some(rhs),
            _ => None,
        })
    }

    // ── useState ──────────────────────────────────────────────────────────────

    #[test]
    fn use_state_destructure() {
        let (cfg, hooks) = parse_and_extract(
            "function Counter() { const [n, setN] = useState(0); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::State { label: 0, init: Expr::Lit(Prim::Int(0)) }));

        let stmts = entry_stmts(&cfg);
        // Temp var must be gone
        assert!(!stmts.iter().any(|s| matches!(s, Stmt::Let { var, .. } if var.starts_with("__arr_"))));
        assert!(matches!(find_let_rhs(stmts, "n"), Some(Expr::StateVal(0))));
        assert!(matches!(find_let_rhs(stmts, "setN"), Some(Expr::StateSetter(0))));
    }

    #[test]
    fn use_state_no_destructure() {
        let (cfg, hooks) = parse_and_extract(
            "function S() { const pair = useState(42); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::State { label: 0, init: Expr::Lit(Prim::Int(42)) }));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "pair"), Some(Expr::StateVal(0))));
    }

    // ── useEffect ────────────────────────────────────────────────────────────

    #[test]
    fn use_effect_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ n }) { useEffect(() => { document.title = String(n); }, [n]); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(
            matches!(&hooks[0], HookEntry::Effect { label: 0, deps: Some(deps), .. } if deps.len() == 1)
        );
        // No ExprStmt for useEffect in the entry block
        assert!(!entry_stmts(&cfg).iter().any(|s| matches!(s, Stmt::ExprStmt(_))));
    }

    #[test]
    fn use_effect_no_deps() {
        let (_, hooks) = parse_and_extract(
            "function Comp() { useEffect(() => {}); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Effect { label: 0, deps: None, .. }));
    }

    // ── useMemo ───────────────────────────────────────────────────────────────

    #[test]
    fn use_memo_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ x }) { const v = useMemo(() => x * 2, [x]); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(
            matches!(&hooks[0], HookEntry::Memo { label: 0, deps, .. } if deps.len() == 1)
        );
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "v"), Some(Expr::MemoVal(0))));
    }

    // ── useCallback ───────────────────────────────────────────────────────────

    #[test]
    fn use_callback_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ onClick }) { const cb = useCallback(() => onClick(), [onClick]); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Callback { label: 0, .. }));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "cb"), Some(Expr::CallbackVal(0))));
    }

    // ── useRef ────────────────────────────────────────────────────────────────

    #[test]
    fn use_ref_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp() { const r = useRef(null); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Ref { label: 0, init: Expr::Lit(Prim::Null) }));
        // useRef result is opaque Lit(Unit)
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "r"), Some(Expr::Lit(Prim::Unit))));
    }

    // ── Custom hook ───────────────────────────────────────────────────────────

    #[test]
    fn custom_hook_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ id }) { const data = useData(id); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Custom { label: 0, name, .. } if name == "useData"));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "data"), Some(Expr::Lit(Prim::Unit))));
    }

    // ── Multiple hooks ────────────────────────────────────────────────────────

    #[test]
    fn multiple_hooks_labeled_in_order() {
        let (cfg, hooks) = parse_and_extract(
            "function App() {
                const [a, setA] = useState(1);
                const [b, setB] = useState(2);
                return <div/>;
            }",
        );
        assert_eq!(hooks.len(), 2);
        assert!(matches!(&hooks[0], HookEntry::State { label: 0, init: Expr::Lit(Prim::Int(1)) }));
        assert!(matches!(&hooks[1], HookEntry::State { label: 1, init: Expr::Lit(Prim::Int(2)) }));

        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "a"), Some(Expr::StateVal(0))));
        assert!(matches!(find_let_rhs(stmts, "setA"), Some(Expr::StateSetter(0))));
        assert!(matches!(find_let_rhs(stmts, "b"), Some(Expr::StateVal(1))));
        assert!(matches!(find_let_rhs(stmts, "setB"), Some(Expr::StateSetter(1))));
    }

    // ── React.useState namespace form ─────────────────────────────────────────

    #[test]
    fn namespaced_hook() {
        let (cfg, hooks) = parse_and_extract(
            "function C() { const [v, setV] = React.useState(0); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::State { label: 0, .. }));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "v"), Some(Expr::StateVal(0))));
        assert!(matches!(find_let_rhs(stmts, "setV"), Some(Expr::StateSetter(0))));
    }

    // ── useReducer ────────────────────────────────────────────────────────────

    #[test]
    fn use_reducer_destructure() {
        let (cfg, hooks) = parse_and_extract(
            "function C() {
                const [state, dispatch] = useReducer(reducer, { count: 0 });
                return <div/>;
            }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Custom { label: 0, name, .. } if name == "useReducer"));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "state"), Some(Expr::StateVal(0))));
        assert!(matches!(find_let_rhs(stmts, "dispatch"), Some(Expr::StateSetter(0))));
    }
}
