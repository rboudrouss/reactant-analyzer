use std::collections::HashSet;

use crate::ir::{
    cfg::{CFG, Terminator},
    expr::Expr,
    stmt::Stmt,
    types::Var,
};

/// Compute free variables of a CFG: variables read anywhere minus variables locally defined.
pub fn compute_free_vars(cfg: &CFG) -> HashSet<Var> {
    let mut used: HashSet<Var> = HashSet::new();
    let mut defined: HashSet<Var> = HashSet::new();

    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, rhs, .. } => {
                    collect_used_vars(rhs, &mut used);
                    defined.insert(var.clone());
                }
                Stmt::Assign { var, rhs, .. } => {
                    collect_used_vars(rhs, &mut used);
                    defined.insert(var.clone());
                }
                Stmt::ExprStmt(e, _) => collect_used_vars(e, &mut used),
            }
        }
        match &block.term {
            Terminator::Branch { cond, .. } => collect_used_vars(cond, &mut used),
            Terminator::Return(expr) => collect_used_vars(expr, &mut used),
            _ => {}
        }
    }

    used.difference(&defined).cloned().collect()
}

/// Root variable a dep expression *covers* in a deps array.
///
/// React compares dep values with `Object.is`; a dep like `memo.content`
/// re-runs the hook whenever that member changes, so at the analyzer's
/// variable granularity it counts as declaring a dependency on `memo`:
/// `[memo.content]` covers uses of `memo`. Peels `FieldAccess` /
/// `IndexAccess` / `TSAnnotated` chains down to the root `Var`.
///
/// Deliberate trade-off (see TODO.md F1): this silences the mismatch case
/// `use(x.a)` with deps `[x.b]`, which needs path-granular free variables
/// (F1b) to detect *precisely* — the variable-granular accidental warning it
/// replaced flagged every correct member dep as missing.
///
/// Returns `None` for deps with no single root variable (literals, calls,
/// state slots — `StateVal` deps are matched by label elsewhere).
pub fn dep_root(expr: &Expr) -> Option<&Var> {
    match expr {
        Expr::Var(v) => Some(v),
        Expr::FieldAccess { obj, .. } => dep_root(obj),
        // `x[i]` re-reads `x`; the index is not what the dep declares.
        Expr::IndexAccess { arr, .. } => dep_root(arr),
        Expr::TSAnnotated(e, _) => dep_root(e),
        _ => None,
    }
}

pub fn collect_used_vars(expr: &Expr, out: &mut HashSet<Var>) {
    match expr {
        Expr::Var(v) => {
            out.insert(v.clone());
        }
        Expr::ObjectLit { fields, .. } => {
            fields.iter().for_each(|(_, v)| collect_used_vars(v, out))
        }
        Expr::ArrayLit { elems, .. } => elems.iter().for_each(|e| collect_used_vars(e, out)),
        Expr::FnLit {
            params, body_cfg, ..
        } => {
            // The lambda's own params shadow outer bindings: they are bound,
            // not free, inside its body (`(open) => !open` reads no outer `open`).
            let mut inner = compute_free_vars(body_cfg);
            for p in params {
                inner.remove(p);
            }
            out.extend(inner);
        }
        Expr::FieldAccess { obj, .. } => collect_used_vars(obj, out),
        Expr::IndexAccess { arr, idx } => {
            collect_used_vars(arr, out);
            collect_used_vars(idx, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_used_vars(lhs, out);
            collect_used_vars(rhs, out);
        }
        Expr::UnaryOp { arg, .. } => collect_used_vars(arg, out),
        Expr::Call { fn_, args } => {
            collect_used_vars(fn_, out);
            args.iter().for_each(|a| collect_used_vars(a, out));
        }
        Expr::CompApp { props, .. } => collect_used_vars(props, out),
        Expr::NativeElem {
            props, children, ..
        } => {
            collect_used_vars(props, out);
            children.iter().for_each(|c| collect_used_vars(c, out));
        }
        Expr::TSAnnotated(e, _) => collect_used_vars(e, out),
        _ => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::cfg::{BasicBlock, CFG, Terminator};
    use crate::ir::expr::Prim;
    use crate::ir::types::ExprId;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn single_return_cfg(expr: Expr) -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(expr),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    fn lambda(params: &[&str], body: Expr) -> Expr {
        Expr::FnLit {
            id: ExprId(0),
            params: params.iter().map(|p| p.to_string()).collect(),
            body_cfg: Arc::new(single_return_cfg(body)),
        }
    }

    #[test]
    fn dep_root_peels_member_chains() {
        let memo = || Box::new(Expr::Var("memo".to_string()));
        assert_eq!(
            dep_root(&Expr::Var("memo".to_string())),
            Some(&"memo".to_string())
        );
        assert_eq!(
            dep_root(&Expr::FieldAccess {
                obj: memo(),
                field: "content".to_string()
            }),
            Some(&"memo".to_string())
        );
        // nested: memo.a.b
        assert_eq!(
            dep_root(&Expr::FieldAccess {
                obj: Box::new(Expr::FieldAccess {
                    obj: memo(),
                    field: "a".to_string()
                }),
                field: "b".to_string()
            }),
            Some(&"memo".to_string())
        );
        assert_eq!(
            dep_root(&Expr::IndexAccess {
                arr: memo(),
                idx: Box::new(Expr::Lit(Prim::Int(0)))
            }),
            Some(&"memo".to_string())
        );
        assert_eq!(dep_root(&Expr::Lit(Prim::Int(1))), None);
        assert_eq!(dep_root(&Expr::StateVal(0)), None);
        assert_eq!(
            dep_root(&Expr::Call {
                fn_: memo(),
                args: vec![]
            }),
            None,
            "a call dep has no stable root"
        );
    }

    #[test]
    fn lambda_param_shadows_outer_binding() {
        // (open) => !open : `open` is bound by the param, not free.
        let cfg = single_return_cfg(lambda(
            &["open"],
            Expr::UnaryOp {
                op: crate::ir::expr::UnaryOp::Not,
                arg: Box::new(Expr::Var("open".to_string())),
            },
        ));
        assert!(compute_free_vars(&cfg).is_empty());
    }

    #[test]
    fn non_shadowed_capture_stays_free() {
        // (x) => !open : `open` captured from outside.
        let cfg = single_return_cfg(lambda(
            &["x"],
            Expr::UnaryOp {
                op: crate::ir::expr::UnaryOp::Not,
                arg: Box::new(Expr::Var("open".to_string())),
            },
        ));
        let free = compute_free_vars(&cfg);
        assert!(free.contains("open"));
    }

    #[test]
    fn shadowing_is_per_lambda_not_global() {
        // f((open) => open, () => open) : second lambda's `open` is free.
        let call = Expr::Call {
            fn_: Box::new(Expr::Var("f".to_string())),
            args: vec![
                lambda(&["open"], Expr::Var("open".to_string())),
                lambda(&[], Expr::Var("open".to_string())),
            ],
        };
        let cfg = single_return_cfg(call);
        let free = compute_free_vars(&cfg);
        assert!(
            free.contains("open"),
            "unshadowed sibling use must stay free"
        );
    }

    #[test]
    fn param_of_inner_lambda_does_not_leak() {
        // outer body also uses `open` directly → free despite inner shadowing.
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
                    crate::ir::stmt::Stmt::ExprStmt(Expr::Var("open".to_string()), None),
                    crate::ir::stmt::Stmt::ExprStmt(
                        lambda(&["open"], Expr::Var("open".to_string())),
                        None,
                    ),
                ],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };
        assert!(compute_free_vars(&cfg).contains("open"));
    }
}
