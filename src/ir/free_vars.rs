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

pub fn collect_used_vars(expr: &Expr, out: &mut HashSet<Var>) {
    match expr {
        Expr::Var(v) => {
            out.insert(v.clone());
        }
        Expr::ObjectLit { fields, .. } => {
            fields.iter().for_each(|(_, v)| collect_used_vars(v, out))
        }
        Expr::ArrayLit { elems, .. } => elems.iter().for_each(|e| collect_used_vars(e, out)),
        Expr::FnLit { body_cfg, .. } => {
            out.extend(compute_free_vars(body_cfg));
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
