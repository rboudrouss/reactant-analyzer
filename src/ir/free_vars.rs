use std::collections::HashSet;
use std::fmt;

use crate::ir::{
    cfg::{CFG, Terminator},
    expr::Expr,
    stmt::Stmt,
    types::Var,
};

/// A read access rooted at a variable, refined by a chain of field names.
///
/// `x` → `{root: x, segments: []}`; `x.a.b` → `{root: x, segments: [a, b]}`.
/// This is the granularity `missing-deps` matches at (TODO.md F1b): a dep
/// `[x.b]` covers a use of `x.a` only if it is a *prefix* of it, so `use(x.a)`
/// with deps `[x.b]` is (correctly) reported while `use(x.b)` with `[x.b]`
/// and `use(x.a)` with `[x]` are not.
///
/// Dynamic member access (`x[i]`) cannot name the touched element, so it
/// collapses to the bare root `{root: x, segments: []}` on the *use* side —
/// "touches all of `x`", coverable only by a whole-`x` dep (never a false
/// negative). On the *dep* side such an access declares nothing (see
/// [`dep_paths`]).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct AccessPath {
    pub root: Var,
    pub segments: Vec<String>,
}

impl fmt::Display for AccessPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.root)?;
        for seg in &self.segments {
            write!(f, ".{seg}")?;
        }
        Ok(())
    }
}

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
                // Writing `obj.f` reads `obj` (and the index); nothing is defined.
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    collect_used_vars(obj, &mut used);
                    if let crate::ir::stmt::MemberKey::Index(idx) = key {
                        collect_used_vars(idx, &mut used);
                    }
                    collect_used_vars(rhs, &mut used);
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

/// Compute free access *paths* of a CFG (see [`AccessPath`]): every read,
/// refined to the member chain actually touched, minus locally-defined roots.
///
/// The root-variable set matches [`compute_free_vars`] exactly; this only adds
/// the field-chain suffix so `missing-deps` can distinguish `x.a` from `x.b`.
pub fn compute_free_paths(cfg: &CFG) -> HashSet<AccessPath> {
    let mut used: HashSet<AccessPath> = HashSet::new();
    let mut defined: HashSet<Var> = HashSet::new();

    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, rhs, .. } => {
                    collect_used_paths(rhs, &mut used);
                    defined.insert(var.clone());
                }
                Stmt::Assign { var, rhs, .. } => {
                    collect_used_paths(rhs, &mut used);
                    defined.insert(var.clone());
                }
                // Writing `obj.f` reads `obj` (and the index); nothing is defined.
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    collect_used_paths(obj, &mut used);
                    if let crate::ir::stmt::MemberKey::Index(idx) = key {
                        collect_used_paths(idx, &mut used);
                    }
                    collect_used_paths(rhs, &mut used);
                }
                Stmt::ExprStmt(e, _) => collect_used_paths(e, &mut used),
            }
        }
        match &block.term {
            Terminator::Branch { cond, .. } => collect_used_paths(cond, &mut used),
            Terminator::Return(expr) => collect_used_paths(expr, &mut used),
            _ => {}
        }
    }

    used.retain(|p| !defined.contains(&p.root));
    used
}

/// Access paths *declared* by a deps array. A dep with a dynamic index
/// (`x[i]`) declares nothing coverable — we can't prove which element it
/// pins — so it is dropped (leaning to a false positive, never a negative).
/// Non-variable deps (literals, calls) yield no path.
pub fn dep_paths(deps: &[Expr]) -> Vec<AccessPath> {
    deps.iter()
        .filter_map(|e| {
            let mut side = Vec::new();
            match extract_path(e, &mut side) {
                Some((root, segments, opaque)) if !opaque => Some(AccessPath { root, segments }),
                _ => None,
            }
        })
        .collect()
}

/// A used path is covered when some declared path is a *prefix* of it:
/// `[x]` covers `x.a`, `[x.a]` covers `x.a` and `x.a.b`, but `[x.b]` covers
/// neither `x.a` nor whole `x`.
pub fn path_covered(used: &AccessPath, declared: &[AccessPath]) -> bool {
    declared.iter().any(|d| {
        d.root == used.root
            && d.segments.len() <= used.segments.len()
            && used.segments[..d.segments.len()] == d.segments[..]
    })
}

/// Extract the maximal member chain rooted at a variable, pushing any
/// off-chain sub-expressions (index expressions, non-variable bases) into
/// `side` for the caller to recurse into. Returns `(root, segments, opaque)`
/// where `opaque` marks a dynamic index encountered in the chain (segments
/// are then meaningless — collapsed to the bare root).
fn extract_path<'e>(e: &'e Expr, side: &mut Vec<&'e Expr>) -> Option<(Var, Vec<String>, bool)> {
    match e {
        Expr::Var(v) => Some((v.clone(), Vec::new(), false)),
        Expr::TSAnnotated(inner, _) => extract_path(inner, side),
        Expr::FieldAccess { obj, field } => {
            extract_path(obj, side).map(|(root, mut segs, opaque)| {
                if !opaque {
                    segs.push(field.clone());
                }
                (root, segs, opaque)
            })
        }
        Expr::IndexAccess { arr, idx } => {
            side.push(idx);
            extract_path(arr, side).map(|(root, _, _)| (root, Vec::new(), true))
        }
        other => {
            side.push(other);
            None
        }
    }
}

fn collect_used_paths(expr: &Expr, out: &mut HashSet<AccessPath>) {
    match expr {
        Expr::Var(_) | Expr::FieldAccess { .. } | Expr::IndexAccess { .. } => {
            let mut side = Vec::new();
            if let Some((root, segments, _)) = extract_path(expr, &mut side) {
                out.insert(AccessPath { root, segments });
            }
            for s in side {
                collect_used_paths(s, out);
            }
        }
        Expr::TSAnnotated(inner, _) => collect_used_paths(inner, out),
        Expr::ObjectLit { fields, .. } => {
            fields.iter().for_each(|(_, v)| collect_used_paths(v, out))
        }
        Expr::ArrayLit { elems, .. } => elems.iter().for_each(|e| collect_used_paths(e, out)),
        Expr::FnLit {
            params, body_cfg, ..
        } => {
            // The lambda's own params shadow outer bindings (same as
            // `collect_used_vars`): subtract them from the captured paths.
            let inner = compute_free_paths(body_cfg);
            out.extend(inner.into_iter().filter(|p| !params.contains(&p.root)));
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_used_paths(lhs, out);
            collect_used_paths(rhs, out);
        }
        Expr::UnaryOp { arg, .. } => collect_used_paths(arg, out),
        Expr::Call { fn_, args } => {
            collect_used_paths(fn_, out);
            args.iter().for_each(|a| collect_used_paths(a, out));
        }
        Expr::CompApp { props, .. } => collect_used_paths(props, out),
        Expr::NativeElem {
            props, children, ..
        } => {
            collect_used_paths(props, out);
            children.iter().for_each(|c| collect_used_paths(c, out));
        }
        _ => {}
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

    fn path(root: &str, segs: &[&str]) -> AccessPath {
        AccessPath {
            root: root.to_string(),
            segments: segs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn field(obj: Expr, name: &str) -> Expr {
        Expr::FieldAccess {
            obj: Box::new(obj),
            field: name.to_string(),
        }
    }

    #[test]
    fn dep_paths_keep_member_chains() {
        // [memo.content] declares memo.content, not whole memo.
        let deps = vec![field(Expr::Var("memo".to_string()), "content")];
        assert_eq!(dep_paths(&deps), vec![path("memo", &["content"])]);
        // [memo.a.b]
        let deps = vec![field(field(Expr::Var("memo".to_string()), "a"), "b")];
        assert_eq!(dep_paths(&deps), vec![path("memo", &["a", "b"])]);
        // [memo] whole var
        assert_eq!(
            dep_paths(&[Expr::Var("memo".to_string())]),
            vec![path("memo", &[])]
        );
    }

    #[test]
    fn dep_paths_drop_dynamic_index_and_non_vars() {
        // [x[i]] — dynamic element, declares nothing coverable.
        let deps = vec![Expr::IndexAccess {
            arr: Box::new(Expr::Var("x".to_string())),
            idx: Box::new(Expr::Var("i".to_string())),
        }];
        assert!(dep_paths(&deps).is_empty());
        // literals / calls
        assert!(dep_paths(&[Expr::Lit(Prim::Int(1))]).is_empty());
        assert!(
            dep_paths(&[Expr::Call {
                fn_: Box::new(Expr::Var("f".to_string())),
                args: vec![],
            }])
            .is_empty()
        );
    }

    #[test]
    fn coverage_is_prefix_based() {
        let decl = vec![path("x", &["a"])];
        assert!(path_covered(&path("x", &["a"]), &decl), "exact");
        assert!(
            path_covered(&path("x", &["a", "b"]), &decl),
            "x.a covers x.a.b"
        );
        assert!(
            !path_covered(&path("x", &["b"]), &decl),
            "x.a does not cover x.b"
        );
        assert!(
            !path_covered(&path("x", &[]), &decl),
            "x.a does not cover whole x"
        );
        // whole-var dep covers any field.
        let whole = vec![path("x", &[])];
        assert!(path_covered(&path("x", &["a"]), &whole));
        assert!(path_covered(&path("x", &[]), &whole));
        // different root never covers.
        assert!(!path_covered(&path("y", &["a"]), &decl));
    }

    #[test]
    fn free_paths_record_member_chain_not_root() {
        // return x.a.b  →  free path x.a.b
        let cfg = single_return_cfg(field(field(Expr::Var("x".to_string()), "a"), "b"));
        let free = compute_free_paths(&cfg);
        assert_eq!(free.len(), 1);
        assert!(free.contains(&path("x", &["a", "b"])));
    }

    #[test]
    fn free_paths_dynamic_index_collapses_to_root() {
        // return x[i].a  →  uses whole x (opaque) AND i
        let cfg = single_return_cfg(field(
            Expr::IndexAccess {
                arr: Box::new(Expr::Var("x".to_string())),
                idx: Box::new(Expr::Var("i".to_string())),
            },
            "a",
        ));
        let free = compute_free_paths(&cfg);
        assert!(
            free.contains(&path("x", &[])),
            "x[i].a touches all of x: {free:?}"
        );
        assert!(
            free.contains(&path("i", &[])),
            "index var is used: {free:?}"
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
