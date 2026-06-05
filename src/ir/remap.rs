use std::sync::Arc;

use crate::ir::{
    cfg::{BasicBlock, CFG, Terminator},
    expr::Expr,
    hooks::HookEntry,
    stmt::Stmt,
    types::HookLabel,
};

/// Add `offset` to every `HookLabel` embedded in an `Expr`.
/// Recurses into all sub-expressions and into `FnLit` bodies.
pub fn remap_expr(expr: Expr, offset: HookLabel) -> Expr {
    if offset == 0 {
        return expr;
    }
    match expr {
        Expr::StateVal(l) => Expr::StateVal(l + offset),
        Expr::StateSetter(l) => Expr::StateSetter(l + offset),
        Expr::MemoVal(l) => Expr::MemoVal(l + offset),
        Expr::CallbackVal(l) => Expr::CallbackVal(l + offset),

        Expr::FnLit {
            id,
            params,
            body_cfg,
        } => {
            let owned = Arc::try_unwrap(body_cfg).unwrap_or_else(|a| (*a).clone());
            Expr::FnLit {
                id,
                params,
                body_cfg: Arc::new(remap_cfg(owned, offset)),
            }
        }

        Expr::ObjectLit { id, fields } => Expr::ObjectLit {
            id,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, remap_expr(v, offset)))
                .collect(),
        },
        Expr::ArrayLit { id, elems } => Expr::ArrayLit {
            id,
            elems: elems.into_iter().map(|e| remap_expr(e, offset)).collect(),
        },

        Expr::FieldAccess { obj, field } => Expr::FieldAccess {
            obj: Box::new(remap_expr(*obj, offset)),
            field,
        },
        Expr::IndexAccess { arr, idx } => Expr::IndexAccess {
            arr: Box::new(remap_expr(*arr, offset)),
            idx: Box::new(remap_expr(*idx, offset)),
        },
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(remap_expr(*lhs, offset)),
            rhs: Box::new(remap_expr(*rhs, offset)),
        },
        Expr::UnaryOp { op, arg } => Expr::UnaryOp {
            op,
            arg: Box::new(remap_expr(*arg, offset)),
        },
        Expr::Call { fn_, args } => Expr::Call {
            fn_: Box::new(remap_expr(*fn_, offset)),
            args: args.into_iter().map(|a| remap_expr(a, offset)).collect(),
        },
        Expr::CompApp { name, props } => Expr::CompApp {
            name,
            props: Box::new(remap_expr(*props, offset)),
        },
        Expr::NativeElem {
            tag,
            props,
            children,
            prop_spans,
        } => Expr::NativeElem {
            tag,
            props: Box::new(remap_expr(*props, offset)),
            children: children
                .into_iter()
                .map(|c| remap_expr(c, offset))
                .collect(),
            prop_spans,
        },
        Expr::TSAnnotated(inner, ts) => Expr::TSAnnotated(Box::new(remap_expr(*inner, offset)), ts),

        // Leaves with no HookLabel or sub-Expr.
        leaf @ (Expr::Lit(_) | Expr::Var(_) | Expr::SummaryVal(_)) => leaf,
    }
}

fn remap_stmt(stmt: Stmt, offset: HookLabel) -> Stmt {
    match stmt {
        Stmt::Let { var, rhs, span } => Stmt::Let {
            var,
            rhs: remap_expr(rhs, offset),
            span,
        },
        Stmt::Assign { var, rhs, span } => Stmt::Assign {
            var,
            rhs: remap_expr(rhs, offset),
            span,
        },
        Stmt::ExprStmt(expr, span) => Stmt::ExprStmt(remap_expr(expr, offset), span),
    }
}

fn remap_terminator(term: Terminator, offset: HookLabel) -> Terminator {
    match term {
        Terminator::Return(e) => Terminator::Return(remap_expr(e, offset)),
        Terminator::Branch { cond, then_, else_ } => Terminator::Branch {
            cond: remap_expr(cond, offset),
            then_,
            else_,
        },
        t @ (Terminator::Jump(_) | Terminator::Unreachable) => t,
    }
}

/// Clone a `CFG`, adding `offset` to every `HookLabel` in all statements and terminators.
/// `BlockId`s are left unchanged.
pub fn remap_cfg(cfg: CFG, offset: HookLabel) -> CFG {
    if offset == 0 {
        return cfg;
    }
    let blocks = cfg
        .blocks
        .into_iter()
        .map(|(id, block)| {
            let stmts = block
                .stmts
                .into_iter()
                .map(|s| remap_stmt(s, offset))
                .collect();
            let term = remap_terminator(block.term, offset);
            (
                id,
                BasicBlock {
                    id: block.id,
                    stmts,
                    term,
                },
            )
        })
        .collect();
    CFG {
        entry: cfg.entry,
        blocks,
        edges: cfg.edges,
    }
}

/// Clone a `Vec<HookEntry>`, adding `offset` to every `HookLabel`
/// (in `label` fields and in embedded `CFG`s and `Expr`s).
pub fn remap_hooks(hooks: Vec<HookEntry>, offset: HookLabel) -> Vec<HookEntry> {
    if offset == 0 {
        return hooks;
    }
    hooks
        .into_iter()
        .map(|h| remap_hook_entry(h, offset))
        .collect()
}

fn remap_hook_entry(entry: HookEntry, offset: HookLabel) -> HookEntry {
    match entry {
        HookEntry::State {
            label,
            init,
            type_hint,
            span,
        } => HookEntry::State {
            label: label + offset,
            init: remap_expr(init, offset),
            type_hint,
            span,
        },
        HookEntry::Effect {
            label,
            body_cfg,
            deps,
            span,
        } => HookEntry::Effect {
            label: label + offset,
            body_cfg: remap_cfg(body_cfg, offset),
            deps: deps.map(|v| v.into_iter().map(|e| remap_expr(e, offset)).collect()),
            span,
        },
        HookEntry::Memo {
            label,
            body_cfg,
            deps,
            span,
        } => HookEntry::Memo {
            label: label + offset,
            body_cfg: remap_cfg(body_cfg, offset),
            deps: deps.into_iter().map(|e| remap_expr(e, offset)).collect(),
            span,
        },
        HookEntry::Callback {
            label,
            body_cfg,
            deps,
            span,
        } => HookEntry::Callback {
            label: label + offset,
            body_cfg: remap_cfg(body_cfg, offset),
            deps: deps.into_iter().map(|e| remap_expr(e, offset)).collect(),
            span,
        },
        HookEntry::Ref { label, init, span } => HookEntry::Ref {
            label: label + offset,
            init: remap_expr(init, offset),
            span,
        },
        HookEntry::Custom {
            label,
            name,
            args,
            deps,
            binding,
            import_source,
            span,
        } => HookEntry::Custom {
            label: label + offset,
            name,
            args: args.into_iter().map(|e| remap_expr(e, offset)).collect(),
            deps: deps.map(|v| v.into_iter().map(|e| remap_expr(e, offset)).collect()),
            binding,
            import_source,
            span,
        },
        HookEntry::Handler {
            label,
            event,
            body_cfg,
            span,
        } => HookEntry::Handler {
            label: label + offset,
            event,
            body_cfg: remap_cfg(body_cfg, offset),
            span,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ir::{
        cfg::{BasicBlock, Terminator},
        expr::{Expr, Prim},
        hooks::HookEntry,
        stmt::Stmt,
    };

    fn single_block_cfg(stmts: Vec<Stmt>, term: Terminator) -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(0, BasicBlock { id: 0, stmts, term });
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    #[test]
    fn remap_zero_is_identity() {
        let e = Expr::StateVal(3);
        assert!(matches!(remap_expr(e, 0), Expr::StateVal(3)));
    }

    #[test]
    fn remap_state_val() {
        assert!(matches!(
            remap_expr(Expr::StateVal(0), 5),
            Expr::StateVal(5)
        ));
        assert!(matches!(
            remap_expr(Expr::StateSetter(2), 10),
            Expr::StateSetter(12)
        ));
        assert!(matches!(remap_expr(Expr::MemoVal(1), 3), Expr::MemoVal(4)));
        assert!(matches!(
            remap_expr(Expr::CallbackVal(0), 7),
            Expr::CallbackVal(7)
        ));
    }

    #[test]
    fn remap_cfg_remaps_stmts_and_term() {
        let cfg = single_block_cfg(
            vec![Stmt::Let {
                var: "x".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            }],
            Terminator::Return(Expr::StateSetter(1)),
        );
        let remapped = remap_cfg(cfg, 10);
        let block = &remapped.blocks[&0];
        assert!(matches!(
            &block.stmts[0],
            Stmt::Let {
                rhs: Expr::StateVal(10),
                ..
            }
        ));
        assert!(matches!(
            &block.term,
            Terminator::Return(Expr::StateSetter(11))
        ));
    }

    #[test]
    fn remap_hooks_remaps_labels_and_body() {
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                type_hint: None,
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: single_block_cfg(
                    vec![Stmt::ExprStmt(Expr::StateSetter(0), None)],
                    Terminator::Return(Expr::Lit(Prim::Unit)),
                ),
                deps: None,
                span: None,
            },
        ];
        let remapped = remap_hooks(hooks, 5);
        assert!(matches!(&remapped[0], HookEntry::State { label: 5, .. }));
        if let HookEntry::Effect {
            label, body_cfg, ..
        } = &remapped[1]
        {
            assert_eq!(*label, 6);
            let block = &body_cfg.blocks[&0];
            assert!(matches!(
                &block.stmts[0],
                Stmt::ExprStmt(Expr::StateSetter(5), _)
            ));
        } else {
            panic!("expected Effect");
        }
    }
}
