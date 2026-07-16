use std::collections::HashMap;
use std::path::PathBuf;

use crate::ir::{
    cfg::{BasicBlock, CFG, Terminator},
    expr::{Expr, Prim},
    hooks::HookEntry,
    source_range::SourceRange,
    stmt::Stmt,
    types::{BlockId, HookLabel},
};

// ── Subscription extraction (addEventListener in effect bodies) ───────────────

/// Scan every effect body for `addEventListener(str, FnLit)` and append a
/// `HookEntry::Handler` for each. Variable callbacks / dynamic names skipped.
pub fn extract_subscriptions(hooks: &mut Vec<HookEntry>, next_label: &mut HookLabel) {
    let n = hooks.len();
    let mut new_handlers: Vec<HookEntry> = Vec::new();
    for i in 0..n {
        if let HookEntry::Effect { body_cfg, .. } = &hooks[i] {
            collect_subscriptions_in_cfg(body_cfg, &mut new_handlers, next_label);
        }
    }
    hooks.extend(new_handlers);
}

fn collect_subscriptions_in_cfg(cfg: &CFG, out: &mut Vec<HookEntry>, next_label: &mut HookLabel) {
    let mut ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let block = &cfg.blocks[&id];
        for stmt in &block.stmts {
            let (expr, span) = match stmt {
                Stmt::Let { rhs, span, .. } => (rhs, *span),
                Stmt::Assign { rhs, span, .. } => (rhs, *span),
                Stmt::ExprStmt(e, span) => (e, *span),
            };
            collect_subscriptions_in_expr(expr, span, out, next_label);
        }
        // Terminator::Return not scanned addEventListener is never a return expr.
    }
}

fn collect_subscriptions_in_expr(
    expr: &Expr,
    stmt_span: Option<SourceRange>,
    out: &mut Vec<HookEntry>,
    next_label: &mut HookLabel,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Expr::FieldAccess { field, .. } = fn_.as_ref()
                && field == "addEventListener"
                && let (
                    Some(Expr::Lit(Prim::String(event_name))),
                    Some(Expr::FnLit { body_cfg, .. }),
                ) = (args.first(), args.get(1))
            {
                let label = *next_label;
                *next_label += 1;
                out.push(HookEntry::Handler {
                    label,
                    event: event_name.clone(),
                    body_cfg: (**body_cfg).clone(),
                    span: stmt_span,
                });
                // Recurse into callee receiver and remaining args (skip arg[1] FnLit body).
                collect_subscriptions_in_expr(fn_, stmt_span, out, next_label);
                for arg in args.iter().skip(2) {
                    collect_subscriptions_in_expr(arg, stmt_span, out, next_label);
                }
                return;
            }
            // Not an addEventListener match recurse into all sub-expressions.
            collect_subscriptions_in_expr(fn_, stmt_span, out, next_label);
            for arg in args {
                collect_subscriptions_in_expr(arg, stmt_span, out, next_label);
            }
        }
        Expr::FieldAccess { obj, .. } => {
            collect_subscriptions_in_expr(obj, stmt_span, out, next_label);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_subscriptions_in_expr(lhs, stmt_span, out, next_label);
            collect_subscriptions_in_expr(rhs, stmt_span, out, next_label);
        }
        Expr::UnaryOp { arg, .. } => {
            collect_subscriptions_in_expr(arg, stmt_span, out, next_label);
        }
        Expr::IndexAccess { arr, idx } => {
            collect_subscriptions_in_expr(arr, stmt_span, out, next_label);
            collect_subscriptions_in_expr(idx, stmt_span, out, next_label);
        }
        Expr::ArrayLit { elems, .. } => {
            for e in elems {
                collect_subscriptions_in_expr(e, stmt_span, out, next_label);
            }
        }
        Expr::ObjectLit { fields, .. } => {
            for (_, v) in fields {
                collect_subscriptions_in_expr(v, stmt_span, out, next_label);
            }
        }
        Expr::TSAnnotated(inner, _) => {
            collect_subscriptions_in_expr(inner, stmt_span, out, next_label);
        }
        // FnLit, Lit, Var, StateVal, StateSetter, MemoVal, CallbackVal, CompApp,
        // NativeElem: leaf or irrelevant in an effect body.
        _ => {}
    }
}

// ── Handler extraction ────────────────────────────────────────────────────────

/// Scan `cfg` for callback props and append each as `HookEntry::Handler`,
/// so their setter writes join the fixpoint (handlers run 0..N times —
/// under-approximating them is an FN class, TODO.md F4).
///
/// Reachability is decided by ESCAPE, not by prop name:
/// - native elements: `on*` events and `ref` (the only native props React
///   invokes);
/// - components: ANY function-valued prop (`onToggle`, `ref`, render props,
///   `action={cb}` — the child may invoke whatever it receives);
/// - values resolve through `handler_body`: inline `FnLit`, a var bound to
///   an `FnLit`, or a `useCallback` (`CallbackVal` — body from its hook
///   entry); JSX inside render-helper closures is scanned too.
///
/// Bare setters as props (`onOpenChange={setOpen}`) are handled by the
/// engine instead (unknown-child havoc in `eval_comp_app`): only the engine
/// knows whether the receiver is analyzable.
pub fn extract_handlers(cfg: &CFG, hooks: &mut Vec<HookEntry>, next_label: &mut HookLabel) {
    // Pre-pass: resolvable handler bodies by variable name.
    // `let cb = () => …` and `let cb = useCallback(…)` (rewritten to
    // `CallbackVal(l)` by extract_hooks, which runs before us).
    let callback_bodies: HashMap<HookLabel, &CFG> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Callback {
                label, body_cfg, ..
            } => Some((*label, body_cfg)),
            _ => None,
        })
        .collect();
    let mut var_bodies: HashMap<&str, CFG> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                match rhs {
                    Expr::FnLit { body_cfg, .. } => {
                        var_bodies.insert(var.as_str(), (**body_cfg).clone());
                    }
                    Expr::CallbackVal(l) => {
                        if let Some(body) = callback_bodies.get(l) {
                            var_bodies.insert(var.as_str(), (*body).clone());
                        }
                    }
                    // Bare setters (`onOpenChange={setOpen}`) are NOT handled
                    // here: whether the receiver may call them with arbitrary
                    // args depends on whether the child is analyzable, which
                    // only the engine knows (see eval_comp_app's unknown-child
                    // havoc). Synthesizing a ⊤-write at lowering time would
                    // clobber the precise inter-component analysis of known
                    // children (e.g. `onChange={setN}` between two analyzed
                    // components).
                    _ => {}
                }
            }
        }
    }

    let mut found: Vec<HookEntry> = Vec::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            let expr = match stmt {
                Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => rhs,
                Stmt::ExprStmt(e, _) => e,
            };
            collect_handlers_in_expr(expr, &var_bodies, &mut found, next_label);
        }
        if let Terminator::Return(e) = &block.term {
            collect_handlers_in_expr(e, &var_bodies, &mut found, next_label);
        }
    }
    hooks.extend(found);
}

/// Resolve one event-prop value to a handler body, if analyzable.
fn handler_body(val: &Expr, var_bodies: &HashMap<&str, CFG>) -> Option<CFG> {
    match val {
        Expr::FnLit { body_cfg, .. } => Some((**body_cfg).clone()),
        Expr::Var(v) => var_bodies.get(v.as_str()).cloned(),
        Expr::TSAnnotated(e, _) => handler_body(e, var_bodies),
        _ => None,
    }
}

fn collect_handlers_in_expr(
    expr: &Expr,
    var_bodies: &HashMap<&str, CFG>,
    found: &mut Vec<HookEntry>,
    next_label: &mut HookLabel,
) {
    match expr {
        Expr::NativeElem {
            props,
            children,
            prop_spans,
            ..
        } => {
            if let Expr::ObjectLit { fields, .. } = props.as_ref() {
                for (name, val) in fields {
                    // `on*` events and `ref` callbacks share invoke semantics
                    // on native elements: React calls them at arbitrary times
                    // (events / mount-unmount). Other native props are DOM
                    // data, never invoked.
                    if is_event_prop(name) || name == "ref" {
                        if let Some(body_cfg) = handler_body(val, var_bodies) {
                            let label = *next_label;
                            *next_label += 1;
                            found.push(HookEntry::Handler {
                                label,
                                event: prop_to_event(name),
                                body_cfg,
                                span: prop_spans.get(name).copied().flatten(),
                            });
                        }
                    } else {
                        collect_handlers_in_expr(val, var_bodies, found, next_label);
                    }
                }
            }
            for child in children {
                collect_handlers_in_expr(child, var_bodies, found, next_label);
            }
        }
        // Component props: ANY function-valued prop handed to a component may
        // be invoked by it at arbitrary times — reachability is decided by
        // escape, not by the prop's name (`ref={captureFrame}`, render props,
        // `action={cb}` all fire; the old `onX` filter was a nominal
        // heuristic, TODO.md B). Only FnLits — inline or locally bound —
        // resolve through `handler_body`, so module-level components passed
        // as props (`component={Page}`) never match. Non-function values
        // recurse for nested JSX (incl. `children`).
        Expr::CompApp { props, .. } => {
            if let Expr::ObjectLit { fields, .. } = props.as_ref() {
                for (name, val) in fields {
                    if let Some(body_cfg) = handler_body(val, var_bodies) {
                        let label = *next_label;
                        *next_label += 1;
                        found.push(HookEntry::Handler {
                            label,
                            event: prop_to_event(name),
                            body_cfg,
                            span: None,
                        });
                    } else {
                        collect_handlers_in_expr(val, var_bodies, found, next_label);
                    }
                }
            }
        }
        // Render helpers (`const renderRow = (x) => <Button onClick={...}/>`)
        // run during render: JSX inside any locally-defined closure is
        // reachable, so its handlers must be extracted too.
        Expr::FnLit { body_cfg, .. } => {
            body_cfg
                .for_each_expr(&mut |e| collect_handlers_in_expr(e, var_bodies, found, next_label));
        }
        // Everything else (TSAnnotated, `children` ArrayLits, object props,
        // conditional temps): JSX can ride anywhere — generic descent.
        other => {
            other.for_each_child(&mut |e| {
                collect_handlers_in_expr(e, var_bodies, found, next_label)
            });
        }
    }
}

fn is_event_prop(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next() == Some('o')
        && chars.next() == Some('n')
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
}

fn prop_to_event(name: &str) -> String {
    // "onClick" → "click",  "onChange" → "change"
    // Non-`onX` callback props (`ref`, render props) keep their name as-is.
    if !is_event_prop(name) {
        return name.to_string();
    }
    let rest = &name[2..];
    let mut s = rest.to_string();
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_lowercase();
    }
    s
}

/// Walk `cfg` in block-id order, extract top-level hook calls, and rewrite
/// affected statements in-place. Returns `(hooks, next_label)`.
/// Destructuring is resolved: `__arr_N[0]` → `StateVal(L)`, `__arr_N[1]` → `StateSetter(L)`.
pub fn extract_hooks(
    cfg: &mut CFG,
    import_map: &HashMap<String, String>,
    resolved_import_map: &HashMap<String, PathBuf>,
) -> (Vec<HookEntry>, HookLabel) {
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
            process_stmt(
                stmt,
                &mut new,
                &mut hooks,
                &mut label,
                &mut state_temps,
                import_map,
                resolved_import_map,
            );
        }

        cfg.blocks.get_mut(&id).unwrap().stmts = new;
    }

    (hooks, label)
}

fn process_stmt(
    stmt: Stmt,
    out: &mut Vec<Stmt>,
    hooks: &mut Vec<HookEntry>,
    label: &mut HookLabel,
    state_temps: &mut HashMap<String, HookLabel>,
    import_map: &HashMap<String, String>,
    resolved_import_map: &HashMap<String, PathBuf>,
) {
    match stmt {
        Stmt::Let {
            var,
            rhs,
            span: stmt_span,
        } => match try_consume_hook_call(rhs) {
            Ok((name, args)) => {
                let lbl = *label;
                *label += 1;
                let is_state_like = matches!(name.as_str(), "useState" | "useReducer");
                let is_arr_temp = var.starts_with("__arr_");

                if let Some(entry) = make_hook_entry(&name, lbl, args, stmt_span) {
                    hooks.push(entry);
                }

                // Record the binding variable, npm import source, and resolved file for Custom hooks.
                if !is_arr_temp
                    && let Some(HookEntry::Custom {
                        binding,
                        import_source,
                        resolved_file,
                        ..
                    }) = hooks.last_mut()
                {
                    *binding = Some(var.clone());
                    *import_source = import_map.get(&name).cloned();
                    *resolved_file = resolved_import_map.get(&name).cloned();
                }

                if is_state_like && is_arr_temp {
                    // Array-destructured useState/useReducer: drop the temp Let,
                    // subsequent IndexAccess stmts will be rewritten by rewrite_expr.
                    state_temps.insert(var, lbl);
                } else {
                    out.push(Stmt::Let {
                        var,
                        rhs: hook_result_expr(&name, lbl),
                        span: None,
                    });
                }
            }
            Err(rhs) => {
                out.push(Stmt::Let {
                    var,
                    rhs: rewrite_expr(rhs, state_temps),
                    span: None,
                });
            }
        },
        Stmt::ExprStmt(expr, stmt_span) => match try_consume_hook_call(expr) {
            Ok((name, args)) => {
                let lbl = *label;
                *label += 1;
                if let Some(entry) = make_hook_entry(&name, lbl, args, stmt_span) {
                    hooks.push(entry);
                }
                // For Custom hooks without a binding, populate import_source + resolved_file.
                if let Some(HookEntry::Custom {
                    import_source,
                    resolved_file,
                    ..
                }) = hooks.last_mut()
                {
                    *import_source = import_map.get(&name).cloned();
                    *resolved_file = resolved_import_map.get(&name).cloned();
                }
                // useEffect and similar void hooks: no stmt emitted.
            }
            Err(expr) => {
                out.push(Stmt::ExprStmt(rewrite_expr(expr, state_temps), None));
            }
        },
        Stmt::Assign { var, rhs, .. } => {
            out.push(Stmt::Assign {
                var,
                rhs: rewrite_expr(rhs, state_temps),
                span: None,
            });
        }
    }
}

// ── Hook call detection ───────────────────────────────────────────────────────

/// Returns `Ok((hook_name, args))` if `expr` is a `use*` call; else `Err(expr)`.
/// A `TSAnnotated` wrapper (`useState<T>(..)`) is looked through — the product
/// value domain (ADR-015) no longer needs the generic-argument type hint.
fn try_consume_hook_call(expr: Expr) -> Result<(String, Vec<Expr>), Expr> {
    match expr {
        Expr::TSAnnotated(inner, ts_type) => {
            if let Expr::Call { fn_, args } = *inner {
                match hook_name_from_callee(&fn_) {
                    Some(name) => Ok((name, args)),
                    None => Err(Expr::TSAnnotated(
                        Box::new(Expr::Call { fn_, args }),
                        ts_type,
                    )),
                }
            } else {
                Err(Expr::TSAnnotated(inner, ts_type))
            }
        }
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

fn make_hook_entry(
    name: &str,
    label: HookLabel,
    args: Vec<Expr>,
    span: Option<SourceRange>,
) -> Option<HookEntry> {
    let mut it = args.into_iter();
    match name {
        "useState" => {
            let init = it.next().unwrap_or(Expr::Lit(Prim::Unit));
            Some(HookEntry::State { label, init, span })
        }
        "useEffect" => {
            let body_cfg = it
                .next()
                .and_then(expr_into_cfg)
                .unwrap_or_else(fallback_cfg);
            let deps = it.next().and_then(expr_into_deps);
            Some(HookEntry::Effect {
                label,
                body_cfg,
                deps,
                span,
            })
        }
        "useMemo" => {
            let body_cfg = it
                .next()
                .and_then(expr_into_cfg)
                .unwrap_or_else(fallback_cfg);
            let deps = it.next().and_then(expr_into_deps).unwrap_or_default();
            Some(HookEntry::Memo {
                label,
                body_cfg,
                deps,
                span,
            })
        }
        "useCallback" => {
            let body_cfg = it
                .next()
                .and_then(expr_into_cfg)
                .unwrap_or_else(fallback_cfg);
            let deps = it.next().and_then(expr_into_deps).unwrap_or_default();
            Some(HookEntry::Callback {
                label,
                body_cfg,
                deps,
                span,
            })
        }
        "useRef" => {
            let init = it.next().unwrap_or(Expr::Lit(Prim::Null));
            Some(HookEntry::Ref { label, init, span })
        }
        "useReducer" => {
            let _reducer = it.next(); // skip reducer fn
            let init = it.next().unwrap_or(Expr::Lit(Prim::Unit));
            Some(HookEntry::State { label, init, span })
        }
        "useLayoutEffect" | "useInsertionEffect" => {
            let body_cfg = it
                .next()
                .and_then(expr_into_cfg)
                .unwrap_or_else(fallback_cfg);
            let deps = it.next().and_then(expr_into_deps);
            Some(HookEntry::Effect {
                label,
                body_cfg,
                deps,
                span,
            })
        }
        _ if name.starts_with("use") => {
            let args: Vec<Expr> = it.collect();
            Some(HookEntry::Custom {
                label,
                name: name.to_string(),
                args,
                deps: None,
                binding: None,
                import_source: None,
                resolved_file: None,
                span,
            })
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
        Expr::FnLit { body_cfg, .. } => {
            // Arc::try_unwrap succeeds if this is the sole owner (always true here since
            // the Expr was just produced by lowering). Fall back to clone for safety.
            Some(std::sync::Arc::try_unwrap(body_cfg).unwrap_or_else(|arc| (*arc).clone()))
        }
        _ => None,
    }
}

fn expr_into_deps(expr: Expr) -> Option<Vec<Expr>> {
    match expr {
        Expr::ArrayLit { elems, .. } => Some(elems),
        _ => None,
    }
}

fn fallback_cfg() -> CFG {
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(
        0,
        BasicBlock {
            id: 0,
            stmts: vec![],
            term: Terminator::Unreachable,
        },
    );
    CFG {
        entry: 0,
        blocks,
        edges: vec![],
    }
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
        Expr::UnaryOp { op, arg } => Expr::UnaryOp {
            op,
            arg: Box::new(rewrite_expr(*arg, state_temps)),
        },
        Expr::Call { fn_, args } => Expr::Call {
            fn_: Box::new(rewrite_expr(*fn_, state_temps)),
            args: args
                .into_iter()
                .map(|a| rewrite_expr(a, state_temps))
                .collect(),
        },
        Expr::ArrayLit { id, elems } => Expr::ArrayLit {
            id,
            elems: elems
                .into_iter()
                .map(|e| rewrite_expr(e, state_temps))
                .collect(),
        },
        Expr::ObjectLit { id, fields } => Expr::ObjectLit {
            id,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, rewrite_expr(v, state_temps)))
                .collect(),
        },
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
                Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| build_cfg(b, &[])),
                _ => None,
            })
            .expect("no function found");
        let (hooks, _) = extract_hooks(&mut cfg, &HashMap::new(), &HashMap::new());
        (cfg, hooks)
    }

    fn entry_stmts(cfg: &CFG) -> &[Stmt] {
        &cfg.blocks[&cfg.entry].stmts
    }

    fn find_let_rhs<'a>(stmts: &'a [Stmt], name: &str) -> Option<&'a Expr> {
        stmts.iter().find_map(|s| match s {
            Stmt::Let { var, rhs, .. } if var == name => Some(rhs),
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
        assert!(matches!(
            &hooks[0],
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                ..
            }
        ));

        let stmts = entry_stmts(&cfg);
        // Temp var must be gone
        assert!(
            !stmts
                .iter()
                .any(|s| matches!(s, Stmt::Let { var, .. } if var.starts_with("__arr_")))
        );
        assert!(matches!(find_let_rhs(stmts, "n"), Some(Expr::StateVal(0))));
        assert!(matches!(
            find_let_rhs(stmts, "setN"),
            Some(Expr::StateSetter(0))
        ));
    }

    #[test]
    fn use_state_no_destructure() {
        let (cfg, hooks) =
            parse_and_extract("function S() { const pair = useState(42); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(
            &hooks[0],
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(42)),
                ..
            }
        ));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "pair"),
            Some(Expr::StateVal(0))
        ));
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
        assert!(
            !entry_stmts(&cfg)
                .iter()
                .any(|s| matches!(s, Stmt::ExprStmt(_, _)))
        );
    }

    #[test]
    fn use_effect_no_deps() {
        let (_, hooks) =
            parse_and_extract("function Comp() { useEffect(() => {}); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(
            &hooks[0],
            HookEntry::Effect {
                label: 0,
                deps: None,
                ..
            }
        ));
    }

    // ── useMemo ───────────────────────────────────────────────────────────────

    #[test]
    fn use_memo_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ x }) { const v = useMemo(() => x * 2, [x]); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Memo { label: 0, deps, .. } if deps.len() == 1));
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
        assert!(matches!(
            find_let_rhs(stmts, "cb"),
            Some(Expr::CallbackVal(0))
        ));
    }

    // ── useRef ────────────────────────────────────────────────────────────────

    #[test]
    fn use_ref_extracted() {
        let (cfg, hooks) =
            parse_and_extract("function Comp() { const r = useRef(null); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(
            &hooks[0],
            HookEntry::Ref {
                label: 0,
                init: Expr::Lit(Prim::Null),
                ..
            }
        ));
        // useRef result is opaque Lit(Unit)
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "r"),
            Some(Expr::Lit(Prim::Unit))
        ));
    }

    // ── Custom hook ───────────────────────────────────────────────────────────

    #[test]
    fn custom_hook_extracted() {
        let (cfg, hooks) =
            parse_and_extract("function Comp({ id }) { const data = useData(id); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Custom { label: 0, name, .. } if name == "useData"));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "data"),
            Some(Expr::Lit(Prim::Unit))
        ));
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
        assert!(matches!(
            &hooks[0],
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(1)),
                ..
            }
        ));
        assert!(matches!(
            &hooks[1],
            HookEntry::State {
                label: 1,
                init: Expr::Lit(Prim::Int(2)),
                ..
            }
        ));

        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "a"), Some(Expr::StateVal(0))));
        assert!(matches!(
            find_let_rhs(stmts, "setA"),
            Some(Expr::StateSetter(0))
        ));
        assert!(matches!(find_let_rhs(stmts, "b"), Some(Expr::StateVal(1))));
        assert!(matches!(
            find_let_rhs(stmts, "setB"),
            Some(Expr::StateSetter(1))
        ));
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
        assert!(matches!(
            find_let_rhs(stmts, "setV"),
            Some(Expr::StateSetter(0))
        ));
    }

    // ── extract_handlers ─────────────────────────────────────────────────────

    fn parse_and_extract_with_handlers(src: &str) -> (CFG, Vec<HookEntry>) {
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
                Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| build_cfg(b, &[])),
                _ => None,
            })
            .expect("no function found");
        let (mut hooks, mut next_label) = extract_hooks(&mut cfg, &HashMap::new(), &HashMap::new());
        extract_handlers(&cfg, &mut hooks, &mut next_label);
        (cfg, hooks)
    }

    #[test]
    fn onclick_handler_extracted() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function Btn() {
                const [n, setN] = useState(0);
                return <button onClick={() => setN(n + 1)}>{n}</button>;
            }",
        );
        // useState is label 0; onClick handler is label 1
        assert_eq!(hooks.len(), 2);
        assert!(matches!(
            &hooks[1],
            HookEntry::Handler { label: 1, event, .. } if event == "click"
        ));
    }

    #[test]
    fn non_fn_event_prop_not_extracted() {
        // onClick={someVar} value is a Var, not FnLit → no Handler entry
        let (_, hooks) = parse_and_extract_with_handlers(
            "function Btn({ onClick }) { return <button onClick={onClick}/>; }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
    }

    #[test]
    fn multiple_handlers_labeled_in_order() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() {
                return <div onMouseEnter={() => {}} onMouseLeave={() => {}}/>;
            }",
        );
        let handlers: Vec<_> = hooks
            .iter()
            .filter(|h| matches!(h, HookEntry::Handler { .. }))
            .collect();
        assert_eq!(handlers.len(), 2);
        assert!(
            matches!(handlers[0], HookEntry::Handler { label: 0, event, .. } if event == "mouseEnter")
        );
        assert!(
            matches!(handlers[1], HookEntry::Handler { label: 1, event, .. } if event == "mouseLeave")
        );
    }

    #[test]
    fn handler_in_nested_jsx() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() {
                return <div><button onClick={() => {}}/></div>;
            }",
        );
        assert_eq!(
            hooks
                .iter()
                .filter(|h| matches!(h, HookEntry::Handler { .. }))
                .count(),
            1
        );
        assert!(matches!(
            hooks.iter().find(|h| matches!(h, HookEntry::Handler { .. })).unwrap(),
            HookEntry::Handler { event, .. } if event == "click"
        ));
    }

    #[test]
    fn on_change_event_name() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() { return <input onChange={() => {}}/>; }",
        );
        assert!(matches!(
            hooks.iter().find(|h| matches!(h, HookEntry::Handler { .. })).unwrap(),
            HookEntry::Handler { event, .. } if event == "change"
        ));
    }

    #[test]
    fn handler_labels_continue_after_hooks() {
        // useState gets label 0, handler gets label 1
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() {
                const [x, setX] = useState(0);
                return <button onClick={() => setX(1)}/>;
            }",
        );
        let handler = hooks
            .iter()
            .find(|h| matches!(h, HookEntry::Handler { .. }))
            .unwrap();
        assert!(matches!(handler, HookEntry::Handler { label: 1, .. }));
    }

    #[test]
    fn handler_span_populated_with_real_line_starts() {
        // Verify that handler spans are non-None when real line_starts are provided.
        // Spans are non-None when real line_starts are provided.
        let src = "function Btn() {\n  return <button onClick={() => {}} />;\n}";
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty());
        let line_starts = crate::ir::compute_line_starts(src);
        let mut cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => {
                    f.body.as_ref().map(|b| build_cfg(b, &line_starts))
                }
                _ => None,
            })
            .expect("no function found");
        let (mut hooks, mut next_label) = extract_hooks(&mut cfg, &HashMap::new(), &HashMap::new());
        extract_handlers(&cfg, &mut hooks, &mut next_label);
        let handler = hooks
            .iter()
            .find(|h| matches!(h, HookEntry::Handler { .. }))
            .expect("no handler found");
        assert!(
            matches!(handler, HookEntry::Handler { span: Some(_), .. }),
            "handler span must be Some when line_starts is non-empty"
        );
    }

    // ── extract_subscriptions ─────────────────────────────────────────────────

    fn parse_and_extract_with_subscriptions(src: &str) -> (CFG, Vec<HookEntry>) {
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
                Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| build_cfg(b, &[])),
                _ => None,
            })
            .expect("no function found");
        let (mut hooks, mut next_label) = extract_hooks(&mut cfg, &HashMap::new(), &HashMap::new());
        extract_handlers(&cfg, &mut hooks, &mut next_label);
        extract_subscriptions(&mut hooks, &mut next_label);
        (cfg, hooks)
    }

    #[test]
    fn addeventlistener_inline_fnlit_extracted() {
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                const [count, setCount] = useState(0);
                useEffect(() => {
                    document.addEventListener('click', () => setCount(count + 1));
                }, [count]);
                return <div/>;
            }",
        );
        let handlers: Vec<_> = hooks
            .iter()
            .filter(|h| matches!(h, HookEntry::Handler { .. }))
            .collect();
        assert_eq!(handlers.len(), 1);
        assert!(matches!(
            handlers[0],
            HookEntry::Handler { event, .. } if event == "click"
        ));
    }

    #[test]
    fn subscription_labels_continue_after_hooks() {
        // useState=0, useEffect=1, subscription handler=2
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                const [n, setN] = useState(0);
                useEffect(() => {
                    window.addEventListener('resize', () => setN(1));
                }, []);
                return <div/>;
            }",
        );
        let handler = hooks
            .iter()
            .find(|h| matches!(h, HookEntry::Handler { .. }))
            .expect("no handler found");
        assert!(matches!(handler, HookEntry::Handler { label: 2, event, .. } if event == "resize"));
    }

    #[test]
    fn addeventlistener_var_event_not_extracted() {
        // Dynamic event name (Var) → acceptable FN, no handler emitted.
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                useEffect(() => {
                    document.addEventListener(eventName, () => {});
                }, []);
                return <div/>;
            }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
    }

    #[test]
    fn addeventlistener_var_callback_not_extracted() {
        // Callback is a Var, not FnLit → acceptable FN, no handler emitted.
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                useEffect(() => {
                    document.addEventListener('click', handler);
                }, []);
                return <div/>;
            }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
    }

    #[test]
    fn multiple_subscriptions_both_extracted() {
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                const [n, setN] = useState(0);
                useEffect(() => {
                    window.addEventListener('mousedown', () => setN(1));
                    window.addEventListener('mouseup', () => setN(0));
                }, []);
                return <div/>;
            }",
        );
        let handlers: Vec<_> = hooks
            .iter()
            .filter(|h| matches!(h, HookEntry::Handler { .. }))
            .collect();
        assert_eq!(handlers.len(), 2);
    }

    #[test]
    fn nested_addeventlistener_in_callback_not_extracted() {
        // addEventListener inside a FnLit body (setTimeout callback) → FnLit is a leaf,
        // not recursed, so the inner addEventListener is not extracted.
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                useEffect(() => {
                    setTimeout(() => {
                        document.addEventListener('click', () => {});
                    }, 100);
                }, []);
                return <div/>;
            }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
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
        assert!(matches!(&hooks[0], HookEntry::State { label: 0, .. }));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "state"),
            Some(Expr::StateVal(0))
        ));
        assert!(matches!(
            find_let_rhs(stmts, "dispatch"),
            Some(Expr::StateSetter(0))
        ));
    }
}
