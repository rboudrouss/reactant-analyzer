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
//!
//! An options object is timed member by member (`ArgTiming::Members`): a
//! member the entry names runs at its time, any other member of the literal
//! keeps the `Unknown` timing, and so does an options argument that is not
//! an object literal.
//!
//! - The response callbacks — `next-safe-action`'s `useAction(action, {
//!   onSuccess, onError, onSettled })`, `swr`'s `useSWR` / `useSWRMutation`
//!   options and `@tanstack/react-query`'s `useMutation({ … })` — run once
//!   the request they follow has answered: deferred. `onExecute` and
//!   `onMutate`, which run when the request starts, are not in the table.
//! - `react-dropzone`'s `useDropzone({ onDrop, … })` drag and drop callbacks
//!   run from the drop zone's DOM listeners, and `onFileDialogCancel` from a
//!   window focus listener: listeners. `onFileDialogOpen` runs in a call of
//!   the result's `open`, which may be made during a render: not in the
//!   table.
//! - `@tanstack/react-table`'s `on…Change` options are not in the table: they
//!   run in a call of the table API that changes their state, and the table
//!   is handed to the components that render it, any of which may call one
//!   during its render.

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
    /// An options object whose named members run at their own time.
    Members(&'static [(&'static str, ArgTiming)]),
}

impl ArgTiming {
    /// The timing of member `name` of an options object timed `self`.
    pub(crate) fn member(self, name: &str) -> Option<ArgTiming> {
        match self {
            ArgTiming::Members(ms) => ms.iter().find(|(m, _)| *m == name).map(|(_, t)| *t),
            _ => None,
        }
    }
}

use ArgTiming::{Deferred, Listener, Members};

/// The response callbacks of a request hook's options.
const RESPONSE: ArgTiming = Members(&[
    ("onSuccess", Deferred),
    ("onError", Deferred),
    ("onSettled", Deferred),
]);

/// `useDropzone`'s DOM-event callbacks.
const DROPZONE: ArgTiming = Members(&[
    ("onDrop", Listener),
    ("onDropAccepted", Listener),
    ("onDropRejected", Listener),
    ("onDragEnter", Listener),
    ("onDragLeave", Listener),
    ("onDragOver", Listener),
    ("onFileDialogCancel", Listener),
]);

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
                ("next-safe-action/hooks" | "next-safe-action", "useAction") => timed(1, RESPONSE),
                ("swr", "useSWR") | ("swr/mutation", "useSWRMutation") => timed(2, RESPONSE),
                ("@tanstack/react-query", "useMutation") => timed(0, RESPONSE),
                ("react-dropzone", "useDropzone") => timed(0, DROPZONE),
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
