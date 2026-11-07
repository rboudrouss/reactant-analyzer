//! When a package hook the analysis has no body for runs a function it is
//! handed.
//!
//! The writer walk reads a left hook's arguments as closures that may run at
//! any time (#262): code it does not see receives them. A few package hooks
//! are documented to run one argument at a known time, and this table names
//! them, keyed by the import specifier the call came from and the hook's own
//! name — a project's own `useDebouncedCallback` proves nothing. Anything
//! the table leaves out keeps the `Unknown` timing.
//!
//! - `use-debounce`'s `useDebouncedCallback(fn, wait)` runs `fn` from a timer
//!   after the calls stop, or synchronously through the result's `flush`, or,
//!   with the `leading` or `maxWait` options, synchronously in a call of the
//!   result. So `fn` is deferred only when no options are passed and the
//!   result is bound to a name every use of which is a call, a `cancel` or an
//!   `isPending`, or an entry of a deps list: a result handed anywhere else
//!   may be flushed there.
//! - `usehooks-ts`'s `useOnClickOutside(ref, handler, …)` runs `handler` from
//!   a document listener and nowhere else: a listener.

use std::collections::HashMap;

use crate::ir::{
    cfg::CFG,
    expr::Expr,
    hooks::HookEntry,
    types::{HookLabel, Var},
};

/// When a hook runs one of its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgTiming {
    /// From a timer, never in the call that schedules it.
    Deferred,
    /// From an event listener.
    Listener,
}

/// The timing of every argument of every hook of `hooks` the expansion left,
/// by label: `None` for an argument the table does not time.
pub(crate) fn left_hook_args(
    render: &CFG,
    hooks: &[HookEntry],
) -> HashMap<HookLabel, Vec<Option<ArgTiming>>> {
    hooks
        .iter()
        .filter_map(|h| {
            let HookEntry::Custom {
                label,
                name,
                args,
                binding,
                import_source,
                ..
            } = h
            else {
                return None;
            };
            let timed = |i: usize, t: ArgTiming| {
                (0..args.len())
                    .map(|j| (j == i).then_some(t))
                    .collect::<Vec<_>>()
            };
            let row = match (import_source.as_deref()?, name.as_str()) {
                ("use-debounce", "useDebouncedCallback")
                    if args.len() <= 2
                        && binding
                            .as_ref()
                            .is_some_and(|v| only_called(v, render, hooks)) =>
                {
                    timed(0, ArgTiming::Deferred)
                }
                ("usehooks-ts", "useOnClickOutside") => timed(1, ArgTiming::Listener),
                _ => return None,
            };
            Some((*label, row))
        })
        .collect()
}

/// The members of a debounced function that never run it.
const INERT_MEMBERS: &[&str] = &["cancel", "isPending"];

/// Every use of `v` in the component — the render, every hook body and every
/// hook argument, nested closures included — is a call of it or of one of
/// its [`INERT_MEMBERS`], or an entry of a deps list (an identity read).
fn only_called(v: &Var, render: &CFG, hooks: &[HookEntry]) -> bool {
    let mut exprs: Vec<&Expr> = Vec::new();
    for h in hooks {
        match h {
            HookEntry::State { init, reducer, .. } => {
                exprs.push(init);
                exprs.extend(reducer);
            }
            HookEntry::Ref { init, .. } => exprs.push(init),
            HookEntry::Custom { args, .. } => exprs.extend(args),
            HookEntry::Effect { .. }
            | HookEntry::Memo { .. }
            | HookEntry::Callback { .. }
            | HookEntry::Handler { .. } => {}
        }
    }
    std::iter::once(render)
        .chain(hooks.iter().filter_map(HookEntry::body_cfg))
        .all(|cfg| cfg_ok(v, cfg))
        && exprs.into_iter().all(|e| expr_ok(v, e))
}

fn cfg_ok(v: &Var, cfg: &CFG) -> bool {
    let mut ok = true;
    cfg.for_each_expr(&mut |e| ok = ok && expr_ok(v, e));
    ok
}

fn expr_ok(v: &Var, e: &Expr) -> bool {
    let is_v = |e: &Expr| matches!(e.peel_ts(), Expr::Var(x) if x == v);
    match e {
        Expr::Var(x) => x != v,
        Expr::FnLit { body_cfg, .. } => cfg_ok(v, body_cfg),
        Expr::Call { fn_, args }
            if is_v(fn_)
                || matches!(fn_.peel_ts(), Expr::FieldAccess { obj, field }
                    if is_v(obj) && INERT_MEMBERS.contains(&field.as_str())) =>
        {
            args.iter().all(|a| expr_ok(v, a))
        }
        _ => {
            let mut ok = true;
            e.for_each_child(&mut |c| ok = ok && expr_ok(v, c));
            ok
        }
    }
}
