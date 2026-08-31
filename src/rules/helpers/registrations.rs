//! The callback-registration relation: which calls inside an effect body hand
//! a callback to something that outlives the render, and how often it fires.
//!
//! Shared by `stale-closure` (which asks what the callback captures) and
//! `missing-cleanup` (which asks whether the registration is ever torn down).
//! It lives here, next to the other CFG/expression scans, because that is what
//! it is; the polarity-typed verdict built on top of it lives in
//! [`crate::rules::api::query`].

use std::collections::HashMap;
use std::sync::Arc;

use crate::ir::{
    SourceRange,
    cfg::{CFG, Terminator},
    expr::Expr,
    stmt::Stmt,
    types::{BlockId, Var},
};

/// How a registered callback re-fires after the render commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Firing {
    /// Fires an unbounded number of times (timer tick, event, subscription).
    Repeating,
    /// Fires once, shortly after registration (timeout, promise, rAF).
    Once,
}

/// A callee that registers a callback surviving the render.
/// `method_only` — must be called as `recv.name(…)`; a bare `then(…)` or
/// `subscribe(…)` global is too ambiguous to claim.
pub(crate) struct Registrar {
    name: &'static str,
    /// Index of the callback argument (`addEventListener('click', cb)` → 1).
    cb_arg: usize,
    firing: Firing,
    method_only: bool,
}

pub(crate) const REGISTRARS: &[Registrar] = &[
    Registrar {
        name: "setInterval",
        cb_arg: 0,
        firing: Firing::Repeating,
        method_only: false,
    },
    Registrar {
        name: "addEventListener",
        cb_arg: 1,
        firing: Firing::Repeating,
        method_only: false,
    },
    Registrar {
        name: "subscribe",
        cb_arg: 0,
        firing: Firing::Repeating,
        method_only: true,
    },
    Registrar {
        name: "on",
        cb_arg: 1,
        firing: Firing::Repeating,
        method_only: true,
    },
    Registrar {
        name: "addListener",
        cb_arg: 1,
        firing: Firing::Repeating,
        method_only: true,
    },
    Registrar {
        name: "setTimeout",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
    },
    Registrar {
        name: "requestAnimationFrame",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
    },
    Registrar {
        name: "requestIdleCallback",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
    },
    Registrar {
        name: "queueMicrotask",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
    },
    Registrar {
        name: "then",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: true,
    },
    Registrar {
        name: "catch",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: true,
    },
    Registrar {
        name: "finally",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: true,
    },
];

/// One callback registration found in an effect body.
pub(crate) struct Registration<'a> {
    /// Display name (`setInterval`, `socket.addEventListener`, `.then`).
    pub display: String,
    pub firing: Firing,
    pub callback: &'a Expr,
    /// Top-level block of the effect body carrying the registration;
    /// `None` when nested in another callback (then never must-reached).
    pub block_id: Option<BlockId>,
    pub span: Option<SourceRange>,
}

/// Match a callee expression against the registrar table.
/// Returns the registrar and its display name.
pub(crate) fn match_registrar(fn_: &Expr) -> Option<(&'static Registrar, String)> {
    let (method, root, is_method) = match fn_.peel_ts() {
        Expr::Var(name) => (name.as_str(), None, false),
        Expr::FieldAccess { obj, field } => {
            let root = match obj.peel_ts() {
                Expr::Var(v) => Some(v.as_str()),
                _ => None,
            };
            (field.as_str(), root, true)
        }
        _ => return None,
    };
    let reg = REGISTRARS
        .iter()
        .find(|r| r.name == method && (!r.method_only || is_method))?;
    let display = match (root, is_method) {
        (Some(r), _) => format!("{r}.{method}"),
        (None, true) => format!(".{method}"),
        (None, false) => method.to_string(),
    };
    Some((reg, display))
}

/// Scan a CFG for callback registrations. `fixed_block`:
/// - `None` → this IS the effect body; each statement carries its own block ID
///   (usable for must-reach);
/// - `Some(b)` → nested context (helper called inline keeps the caller's
///   block, a callback body gets `Some(None)`).
pub(crate) fn collect_registrations<'a>(
    cfg: &'a CFG,
    fn_bodies: &'a HashMap<Var, Arc<CFG>>,
    depth: usize,
    fixed_block: Option<Option<BlockId>>,
    out: &mut Vec<Registration<'a>>,
) {
    for (&bid, block) in &cfg.blocks {
        let block_id = fixed_block.unwrap_or(Some(bid));
        for stmt in &block.stmts {
            let (expr, span) = match stmt {
                Stmt::ExprStmt(e, span) => (e, *span),
                Stmt::Let { rhs, span, .. }
                | Stmt::Assign { rhs, span, .. }
                | Stmt::MemberWrite { rhs, span, .. } => (rhs, *span),
            };
            registrations_in_expr(expr, span, block_id, fn_bodies, depth, out);
        }
        match &block.term {
            Terminator::Return(e) | Terminator::Branch { cond: e, .. } => {
                registrations_in_expr(e, None, block_id, fn_bodies, depth, out);
            }
            _ => {}
        }
    }
}

fn registrations_in_expr<'a>(
    expr: &'a Expr,
    span: Option<SourceRange>,
    block_id: Option<BlockId>,
    fn_bodies: &'a HashMap<Var, Arc<CFG>>,
    depth: usize,
    out: &mut Vec<Registration<'a>>,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Some((reg, display)) = match_registrar(fn_)
                && let Some(cb) = args.get(reg.cb_arg)
            {
                out.push(Registration {
                    display,
                    firing: reg.firing,
                    callback: cb,
                    block_id,
                    span,
                });
            }
            // Direct call of a locally-bound helper executes inline: the
            // registration inside it happens on the caller's block (B6).
            if depth > 0
                && let Expr::Var(name) = fn_.peel_ts()
                && let Some(body) = fn_bodies.get(name)
            {
                collect_registrations(body, fn_bodies, depth - 1, Some(block_id), out);
            }
            // Descend: chained receivers (`p.then(a).then(b)`), callback
            // bodies (a registration inside another callback is never
            // must-reached), plain args.
            registrations_in_expr(fn_, span, block_id, fn_bodies, depth, out);
            for arg in args {
                match arg {
                    Expr::FnLit { body_cfg, .. } if depth > 0 => {
                        collect_registrations(body_cfg, fn_bodies, depth - 1, Some(None), out);
                    }
                    _ => registrations_in_expr(arg, span, block_id, fn_bodies, depth, out),
                }
            }
        }
        other => {
            other.for_each_child(&mut |c| {
                registrations_in_expr(c, span, block_id, fn_bodies, depth, out)
            });
        }
    }
}
