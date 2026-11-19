use std::sync::Arc;

use crate::ir::{
    cfg::{BasicBlock, CFG, Terminator},
    expr::{CALL_SITES, Expr},
    hooks::HookEntry,
    stmt::{MemberKey, Stmt},
    types::{ExprId, HookLabel},
};

/// What grafting a callee into a caller shifts.
///
/// Both are identity supplies the splice hands the copy it makes, and both are
/// needed for the same reason: the caller and the callee number their own
/// things from zero. Labels were shifted from the start; allocation sites were
/// not, so a callee's `ObjectLit` shared a heap entry with the caller's and
/// answered its member reads (#134). One carrier, so a new splice site cannot
/// remember one and forget the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Offsets {
    pub labels: HookLabel,
    /// Added to every allocation-site [`ExprId`]. Unique per splice: the same
    /// callee inlined twice is two allocation sites, not one.
    pub ids: u64,
    /// Added to every call site ([`Expr::Call`]'s `id`), for the same reason:
    /// the same callee inlined twice performs its calls at two places. Calls
    /// have a cursor of their own ([`call_id_span`]), so a graft's call
    /// range moves no allocation site.
    pub calls: u64,
}

impl Offsets {
    /// Shift labels only — for a remap that is not a graft (a hook table
    /// renumbered in place keeps its own allocation sites).
    pub fn labels(labels: HookLabel) -> Self {
        Offsets {
            labels,
            ids: 0,
            calls: 0,
        }
    }

    fn is_identity(self) -> bool {
        self.labels == 0 && self.ids == 0 && self.calls == 0
    }
}

/// How many call sites a graft of `cfg` occupies: one past its largest call
/// id above [`CALL_SITES`], nested function bodies included; 0 for a body
/// that calls nothing. The call-site counterpart of [`alloc_id_span`].
pub fn call_id_span(cfg: &CFG) -> u64 {
    id_span(cfg_exprs(cfg), call_id)
}

/// A call's id above [`CALL_SITES`].
fn call_id(e: &Expr) -> Option<u64> {
    match e {
        Expr::Call { id, .. } => Some(id.0.saturating_sub(CALL_SITES)),
        _ => None,
    }
}

/// An allocation site's id.
fn alloc_id(e: &Expr) -> Option<u64> {
    match e {
        Expr::ObjectLit { id, .. }
        | Expr::ArrayLit { id, .. }
        | Expr::Arguments { id, .. }
        | Expr::New { id, .. }
        | Expr::FnLit { id, .. } => Some(id.0),
        _ => None,
    }
}

/// The top-level expressions of `cfg`.
fn cfg_exprs(cfg: &CFG) -> Vec<&Expr> {
    let mut out = Vec::new();
    cfg.for_each_expr(&mut |e| out.push(e));
    out
}

/// One past the largest id `pick` reads off `exprs`, their sub-expressions
/// and their function literals' bodies.
fn id_span<'a>(exprs: impl IntoIterator<Item = &'a Expr>, pick: fn(&Expr) -> Option<u64>) -> u64 {
    fn walk(e: &Expr, pick: fn(&Expr) -> Option<u64>, max: &mut Option<u64>, depth: usize) {
        if depth == 0 {
            return;
        }
        if let Some(n) = pick(e) {
            *max = Some(max.map_or(n, |m| m.max(n)));
        }
        if let Expr::FnLit { body_cfg, .. } = e {
            body_cfg.for_each_expr(&mut |inner| walk(inner, pick, max, depth - 1));
        }
        e.for_each_child(&mut |c| walk(c, pick, max, depth));
    }
    let mut max: Option<u64> = None;
    // A `FnLit` body is a CFG of its own; the bound keeps a pathological
    // nesting from recursing without end, and no real body approaches it.
    for e in exprs {
        walk(e, pick, &mut max, 64);
    }
    max.map_or(0, |m| m + 1)
}

/// Every expression a hook table holds: each body's, and what an entry holds
/// beside its body — an initializer, a reducer, a deps list, a custom hook's
/// arguments. What [`remap_hooks`] shifts.
fn hook_exprs(hooks: &[HookEntry]) -> Vec<&Expr> {
    fn deps(d: &crate::ir::hooks::DepsArg) -> Vec<&Expr> {
        match d {
            crate::ir::hooks::DepsArg::List(l) => l.as_slice().iter().collect(),
            crate::ir::hooks::DepsArg::Opaque(e) => vec![e],
            crate::ir::hooks::DepsArg::Absent => Vec::new(),
        }
    }
    let mut out = Vec::new();
    for h in hooks {
        if let Some(body) = h.body_cfg() {
            out.extend(cfg_exprs(body));
        }
        match h {
            HookEntry::State { init, reducer, .. } => {
                out.push(init);
                out.extend(reducer.as_ref());
            }
            HookEntry::Ref { init, .. } => out.push(init),
            HookEntry::Effect { deps: d, .. }
            | HookEntry::Memo { deps: d, .. }
            | HookEntry::Callback { deps: d, .. } => out.extend(deps(d)),
            HookEntry::Custom { args, deps: d, .. } => {
                out.extend(args);
                out.extend(deps(d));
            }
            HookEntry::Handler { .. } => {}
        }
    }
    out
}

/// The allocation-id and call-id widths of a body and the hook table grafted
/// with it — every id [`remap_cfg`] and [`remap_hooks`] shift: a graft lands
/// above all of them, an argument of a custom hook the table keeps included.
pub fn graft_spans(cfg: &CFG, hooks: &[HookEntry]) -> (u64, u64) {
    let mut exprs = cfg_exprs(cfg);
    exprs.extend(hook_exprs(hooks));
    (
        id_span(exprs.iter().copied(), alloc_id),
        id_span(exprs.iter().copied(), call_id),
    )
}

/// One past the largest allocation-site `ExprId` in `cfg`, nested function
/// bodies included — the width of the id range a graft of `cfg` occupies.
///
/// A splice adds this to its running cursor so the next graft lands above it,
/// which is what makes the *same* callee inlined twice two allocation sites.
pub fn alloc_id_span(cfg: &CFG) -> u64 {
    id_span(cfg_exprs(cfg), alloc_id)
}

/// Add `off` to every `HookLabel` and every allocation-site `ExprId` embedded
/// in an `Expr`. Recurses into all sub-expressions and into `FnLit` bodies.
pub fn remap_expr(expr: Expr, off: Offsets) -> Expr {
    if off.is_identity() {
        return expr;
    }
    let offset = off.labels;
    let shift = |id: ExprId| ExprId(id.0 + off.ids);
    match expr {
        Expr::StateVal(l) => Expr::StateVal(l + offset),
        Expr::StateSetter(l) => Expr::StateSetter(l + offset),
        Expr::MemoVal(l) => Expr::MemoVal(l + offset),
        Expr::CallbackVal(l) => Expr::CallbackVal(l + offset),
        Expr::HookMarker(l, v) => Expr::HookMarker(l + offset, v),

        Expr::FnLit {
            id,
            params,
            body_cfg,
        } => {
            let owned = Arc::try_unwrap(body_cfg).unwrap_or_else(|a| (*a).clone());
            Expr::FnLit {
                id: shift(id),
                params,
                body_cfg: Arc::new(remap_cfg(owned, off)),
            }
        }

        Expr::ObjectLit { id, fields } => Expr::ObjectLit {
            id: shift(id),
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, remap_expr(v, off)))
                .collect(),
        },
        Expr::ArrayLit {
            id,
            elems,
            arity,
            spread_at,
        } => Expr::ArrayLit {
            id: shift(id),
            elems: elems.into_iter().map(|e| remap_expr(e, off)).collect(),
            arity,
            spread_at,
        },

        Expr::Arguments { id, from } => Expr::Arguments {
            id: shift(id),
            from,
        },

        Expr::FieldAccess { obj, field } => Expr::FieldAccess {
            obj: Box::new(remap_expr(*obj, off)),
            field,
        },
        Expr::IndexAccess { arr, idx } => Expr::IndexAccess {
            arr: Box::new(remap_expr(*arr, off)),
            idx: Box::new(remap_expr(*idx, off)),
        },
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(remap_expr(*lhs, off)),
            rhs: Box::new(remap_expr(*rhs, off)),
        },
        Expr::UnaryOp { op, arg } => Expr::UnaryOp {
            op,
            arg: Box::new(remap_expr(*arg, off)),
        },
        Expr::Call { id, fn_, args } => Expr::Call {
            id: ExprId(id.0 + off.calls),
            fn_: Box::new(remap_expr(*fn_, off)),
            args: args.into_iter().map(|a| remap_expr(a, off)).collect(),
        },
        Expr::New {
            id,
            fn_,
            args,
            ctor,
            on_this,
        } => Expr::New {
            id: shift(id),
            fn_: Box::new(remap_expr(*fn_, off)),
            args: args.into_iter().map(|a| remap_expr(a, off)).collect(),
            ctor: ctor.map(|c| Box::new(remap_expr(*c, off))),
            on_this,
        },
        Expr::CompApp {
            name,
            props,
            span,
            origin,
        } => Expr::CompApp {
            name,
            props: Box::new(remap_expr(*props, off)),
            span,
            origin,
        },
        Expr::NativeElem {
            tag,
            props,
            children,
            span,
            prop_spans,
        } => Expr::NativeElem {
            tag,
            props: Box::new(remap_expr(*props, off)),
            children: children.into_iter().map(|c| remap_expr(c, off)).collect(),
            span,
            prop_spans,
        },
        Expr::TSAnnotated(inner) => Expr::TSAnnotated(Box::new(remap_expr(*inner, off))),
        Expr::Spread(inner) => Expr::Spread(Box::new(remap_expr(*inner, off))),

        // Leaves with no HookLabel or sub-Expr.
        leaf @ (Expr::Lit(_) | Expr::Var(_) | Expr::SummaryVal(_)) => leaf,
    }
}

fn remap_stmt(stmt: Stmt, off: Offsets) -> Stmt {
    match stmt {
        Stmt::Let { var, rhs, span } => Stmt::Let {
            var,
            rhs: remap_expr(rhs, off),
            span,
        },
        Stmt::Assign { var, rhs, span } => Stmt::Assign {
            var,
            rhs: remap_expr(rhs, off),
            span,
        },
        Stmt::MemberWrite {
            obj,
            key,
            rhs,
            span,
        } => Stmt::MemberWrite {
            obj: remap_expr(obj, off),
            key: match key {
                MemberKey::Field(f) => MemberKey::Field(f),
                MemberKey::Index(idx) => MemberKey::Index(remap_expr(idx, off)),
            },
            rhs: remap_expr(rhs, off),
            span,
        },
        Stmt::ExprStmt(expr, span) => Stmt::ExprStmt(remap_expr(expr, off), span),
    }
}

fn remap_terminator(term: Terminator, off: Offsets) -> Terminator {
    match term {
        Terminator::Return(e, span) => Terminator::Return(remap_expr(e, off), span),
        Terminator::Branch {
            cond,
            then_,
            else_,
            span,
            chain_link,
        } => Terminator::Branch {
            cond: remap_expr(cond, off),
            then_,
            else_,
            span,
            chain_link,
        },
        t @ (Terminator::Jump(_) | Terminator::Unreachable) => t,
    }
}

/// Clone a `CFG`, applying `off` to every `HookLabel` and allocation-site
/// `ExprId` in all statements and terminators. `BlockId`s are left unchanged.
pub fn remap_cfg(cfg: CFG, off: Offsets) -> CFG {
    if off.is_identity() {
        return cfg;
    }
    let blocks = cfg
        .blocks
        .into_iter()
        .map(|(id, block)| {
            let stmts = block
                .stmts
                .into_iter()
                .map(|s| remap_stmt(s, off))
                .collect();
            let term = remap_terminator(block.term, off);
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

/// Clone a `Vec<HookEntry>`, applying `off` to every `HookLabel` (in `label`
/// fields and in embedded `CFG`s and `Expr`s) and to every allocation site.
pub fn remap_hooks(hooks: Vec<HookEntry>, off: Offsets) -> Vec<HookEntry> {
    if off.is_identity() {
        return hooks;
    }
    hooks
        .into_iter()
        .map(|h| remap_hook_entry(h, off))
        .collect()
}

fn remap_hook_entry(entry: HookEntry, off: Offsets) -> HookEntry {
    match entry {
        HookEntry::State {
            label,
            init,
            reducer,
            span,
        } => HookEntry::State {
            label: label + off.labels,
            init: remap_expr(init, off),
            reducer: reducer.map(|r| remap_expr(r, off)),
            span,
        },
        HookEntry::Effect {
            label,
            body_cfg,
            deps,
            span,
        } => HookEntry::Effect {
            label: label + off.labels,
            body_cfg: remap_cfg(body_cfg, off),
            deps: deps.map_exprs(|e| remap_expr(e, off)),
            span,
        },
        HookEntry::Memo {
            label,
            body_cfg,
            deps,
            span,
        } => HookEntry::Memo {
            label: label + off.labels,
            body_cfg: remap_cfg(body_cfg, off),
            deps: deps.map_exprs(|e| remap_expr(e, off)),
            span,
        },
        HookEntry::Callback {
            label,
            body_cfg,
            params,
            literal,
            deps,
            span,
        } => HookEntry::Callback {
            label: label + off.labels,
            body_cfg: remap_cfg(body_cfg, off),
            params,
            literal,
            deps: deps.map_exprs(|e| remap_expr(e, off)),
            span,
        },
        HookEntry::Ref { label, init, span } => HookEntry::Ref {
            label: label + off.labels,
            init: remap_expr(init, off),
            span,
        },
        HookEntry::Custom {
            label,
            name,
            args,
            deps,
            binding,
            import_source,
            resolved_file,
            span,
        } => HookEntry::Custom {
            label: label + off.labels,
            name,
            args: args.into_iter().map(|e| remap_expr(e, off)).collect(),
            deps: deps.map_exprs(|e| remap_expr(e, off)),
            binding,
            import_source,
            resolved_file,
            span,
        },
        HookEntry::Handler {
            label,
            event,
            body_cfg,
            params,
            span,
            dom_event,
        } => HookEntry::Handler {
            label: label + off.labels,
            event,
            body_cfg: remap_cfg(body_cfg, off),
            params,
            span,
            dom_event,
        },
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use crate::ir::hooks::DepsArg;
    use crate::ir::{
        cfg::Terminator,
        expr::{Expr, Prim},
        hooks::HookEntry,
        stmt::Stmt,
    };

    fn single_block_cfg(stmts: Vec<Stmt>, term: Terminator) -> CFG {
        crate::test_support::single_block_cfg_term(stmts, term)
    }

    #[test]
    fn remap_zero_is_identity() {
        let e = Expr::StateVal(3);
        assert!(matches!(
            remap_expr(e, Offsets::labels(0)),
            Expr::StateVal(3)
        ));
    }

    #[test]
    fn remap_state_val() {
        assert!(matches!(
            remap_expr(Expr::StateVal(0), Offsets::labels(5)),
            Expr::StateVal(5)
        ));
        assert!(matches!(
            remap_expr(Expr::StateSetter(2), Offsets::labels(10)),
            Expr::StateSetter(12)
        ));
        assert!(matches!(
            remap_expr(Expr::MemoVal(1), Offsets::labels(3)),
            Expr::MemoVal(4)
        ));
        assert!(matches!(
            remap_expr(Expr::CallbackVal(0), Offsets::labels(7)),
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
            Terminator::Return(Expr::StateSetter(1), None),
        );
        let remapped = remap_cfg(cfg, Offsets::labels(10));
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
            Terminator::Return(Expr::StateSetter(11), None)
        ));
    }

    #[test]
    fn remap_hooks_remaps_labels_and_body() {
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                reducer: None,
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: single_block_cfg(
                    vec![Stmt::ExprStmt(Expr::StateSetter(0), None)],
                    Terminator::Return(Expr::Lit(Prim::Unit), None),
                ),
                deps: DepsArg::Absent,
                span: None,
            },
        ];
        let remapped = remap_hooks(hooks, Offsets::labels(5));
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
