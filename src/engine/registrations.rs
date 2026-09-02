//! The callback-registration relation (ADR-034): which calls in an effect body
//! hand a callback to something that outlives the effect, how often it
//! re-fires, when it can run, and whether the effect's cleanup takes it back.
//!
//! One table, three readers. `stale-closure` asks what a registered callback
//! captures, `missing-cleanup` asks whether a repeating registration is ever
//! torn down, and the slot-writer walk asks which phase a callback handed to
//! one of these calls runs in. Before this module the first two shared a scan
//! in `rules::helpers` and the third kept its own name lists — two whitelists
//! overlapping on timers and promise continuations, free to drift.
//!
//! Computed once at convergence and stored on
//! [`AnalysisResult`](crate::engine::AnalysisResult), the ADR-027 §1 template.
//!
//! **Effect bodies only.** That is the scope both native consumers ask for and
//! the one the Tier-A anchor exposes; a registration in a `useCallback` body is
//! a real thing with no reader, so it stays out until one exists.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ir::{
    SourceRange,
    cfg::{CFG, Terminator},
    expr::Expr,
    hooks::HookEntry,
    stmt::Stmt,
    types::{BlockId, HookLabel, Var},
};

/// How a registered callback re-fires after the registering call returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firing {
    /// Fires an unbounded number of times (timer tick, event, subscription).
    Repeating,
    /// Fires once, shortly after registration (timeout, promise, rAF).
    Once,
}

/// When a registered callback can run, relative to the React phases.
///
/// This is the phase *summary* ADR-027 §2 promised and never shipped: a
/// registration argument used to fall to ⊤ in the slot-writer walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// A timer, a microtask, a promise continuation: the callback runs on a
    /// later turn of the event loop, provably outside every React phase.
    Deferred,
    /// An external event. The DOM has no synchronous dispatch from
    /// `addEventListener`, so the callback provably does not run during the
    /// registering call.
    Handler,
    /// The registrar may invoke the callback synchronously — an RxJS
    /// `BehaviorSubject` emits to a new subscriber on the spot — so nothing
    /// about the timing is proven and the walk keeps ⊤.
    Unknown,
}

/// A callee that registers a callback surviving the call.
///
/// `method_only` — must be called as `recv.name(…)`; a bare `then(…)` or
/// `subscribe(…)` global is too ambiguous to claim.
#[derive(Debug)]
pub struct Registrar {
    pub name: &'static str,
    /// Index of the callback argument (`addEventListener('click', cb)` → 1).
    pub cb_arg: usize,
    pub firing: Firing,
    pub method_only: bool,
    pub timing: Timing,
    /// Calls that undo this registration. Empty when the registration cannot
    /// be taken back (`queueMicrotask`, a promise continuation).
    pub teardown: &'static [&'static str],
}

/// The one registrar table. The former `REGISTRARS` in `rules::helpers` and the
/// slot-writer walk's `DEFERRING_GLOBALS` / `DEFERRING_METHODS` were two lists
/// of overlapping names with two different jobs; they are one list with two
/// columns now.
pub const REGISTRARS: &[Registrar] = &[
    Registrar {
        name: "setInterval",
        cb_arg: 0,
        firing: Firing::Repeating,
        method_only: false,
        timing: Timing::Deferred,
        teardown: &["clearInterval"],
    },
    Registrar {
        name: "addEventListener",
        cb_arg: 1,
        firing: Firing::Repeating,
        method_only: false,
        timing: Timing::Handler,
        teardown: &["removeEventListener"],
    },
    Registrar {
        name: "subscribe",
        cb_arg: 0,
        firing: Firing::Repeating,
        method_only: true,
        timing: Timing::Unknown,
        teardown: &["unsubscribe"],
    },
    Registrar {
        name: "on",
        cb_arg: 1,
        firing: Firing::Repeating,
        method_only: true,
        timing: Timing::Unknown,
        teardown: &["off", "removeListener"],
    },
    Registrar {
        name: "addListener",
        cb_arg: 1,
        firing: Firing::Repeating,
        method_only: true,
        timing: Timing::Unknown,
        teardown: &["removeListener"],
    },
    Registrar {
        name: "setTimeout",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
        timing: Timing::Deferred,
        teardown: &["clearTimeout"],
    },
    Registrar {
        name: "setImmediate",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
        timing: Timing::Deferred,
        teardown: &["clearImmediate"],
    },
    Registrar {
        name: "requestAnimationFrame",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
        timing: Timing::Deferred,
        teardown: &["cancelAnimationFrame"],
    },
    Registrar {
        name: "requestIdleCallback",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
        timing: Timing::Deferred,
        teardown: &["cancelIdleCallback"],
    },
    Registrar {
        name: "queueMicrotask",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: false,
        timing: Timing::Deferred,
        teardown: &[],
    },
    Registrar {
        name: "then",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: true,
        timing: Timing::Deferred,
        teardown: &[],
    },
    Registrar {
        name: "catch",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: true,
        timing: Timing::Deferred,
        teardown: &[],
    },
    Registrar {
        name: "finally",
        cb_arg: 0,
        firing: Firing::Once,
        method_only: true,
        timing: Timing::Deferred,
        teardown: &[],
    },
];

/// Whether the effect's cleanup takes a registration back.
///
/// Three-valued for the same reason the cleanup verdict is: the unresolvable
/// case folds to the **may-unpaired** side, so it fires downstream and never
/// certifies a teardown that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pairing {
    /// A cleanup this walk could read calls one of the registrar's teardown
    /// names holding the same listener binding. The only value that is a claim.
    Paired,
    /// A readable cleanup with no such call, or no cleanup at all.
    Unpaired,
    /// A cleanup the walk cannot read, or a listener that is not a resolvable
    /// binding — the teardown may be there and may not.
    Unknown,
}

impl Pairing {
    /// `true` unless the teardown is proven. What a rule that fires on a
    /// missing teardown must ask: `Unknown` is a may-fact on the firing side.
    pub fn may_be_unpaired(self) -> bool {
        self != Pairing::Paired
    }
}

/// One callback registration in an effect body.
#[derive(Debug, Clone)]
pub struct Registration {
    /// The effect whose body carries the registration.
    pub effect: HookLabel,
    /// Display name (`setInterval`, `socket.addEventListener`, `.then`).
    pub display: String,
    /// The table row's name — the stable key `display` is not.
    pub registrar: &'static str,
    pub firing: Firing,
    pub timing: Timing,
    /// The callback argument as written. Cloning an `FnLit` bumps an `Arc`:
    /// the body is not copied.
    pub callback: Expr,
    /// Top-level block of the effect body carrying the registration; `None`
    /// when nested in another callback (then never must-reached).
    pub block_id: Option<BlockId>,
    pub span: Option<SourceRange>,
    pub pairing: Pairing,
}

/// Does this callee **un**register — `removeEventListener`, `unsubscribe`,
/// `clearInterval` and the rest of the table's `teardown` column?
///
/// A teardown takes the callback back; it never calls it. The walk therefore
/// does not descend a function argument in this position, which is what keeps a
/// registered listener's writes from also producing a ⊤ row from the cleanup
/// that removes them (ADR-034 §5). Like every other reading off this table it
/// trusts the name — but unlike the registrar side, trusting it here *narrows*,
/// so the set is deliberately only the teardown partners of registrars already
/// in the table.
pub fn is_teardown(fn_: &Expr) -> bool {
    let name = match fn_.peel_ts() {
        Expr::Var(n) => n.as_str(),
        Expr::FieldAccess { field, .. } => field.as_str(),
        _ => return false,
    };
    REGISTRARS.iter().any(|r| r.teardown.contains(&name))
}

/// Match a callee expression against the registrar table, returning the row and
/// a display name for it.
pub fn match_registrar(fn_: &Expr) -> Option<(&'static Registrar, String)> {
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

// ── The relation ──────────────────────────────────────────────────────────────

/// Collect every registration in the component's effect bodies, each row
/// carrying its pairing verdict against that effect's own cleanup.
///
/// Depth 2 is what both former native scans used: a registration one helper
/// call or one callback deep still counts, nothing deeper does.
pub fn collect_registrations(render_cfg: &CFG, hooks: &[HookEntry]) -> Vec<Registration> {
    let render_fns = crate::engine::setters::collect_fn_bindings(render_cfg);
    let mut out = Vec::new();

    for hook in hooks {
        let HookEntry::Effect {
            label, body_cfg, ..
        } = hook
        else {
            continue;
        };
        let mut fns = crate::engine::setters::collect_fn_bindings(body_cfg);
        for (k, v) in &render_fns {
            fns.entry(k.clone()).or_insert_with(|| Arc::clone(v));
        }
        let mut rows = Vec::new();
        scan_cfg(body_cfg, &fns, 2, None, *label, &mut rows);
        let cleanups = cleanup_bodies(body_cfg);
        for row in &mut rows {
            row.pairing = pair(row, &cleanups, &fns);
        }
        out.append(&mut rows);
    }
    out
}

/// What an effect body hands back as a teardown.
enum Cleanups<'c> {
    /// No exit returns anything: there is provably nothing to pair with.
    None,
    /// Bodies the walk could read. Non-empty.
    Bodies(Vec<&'c CFG>),
    /// Some exit returns something unreadable — a call result, an
    /// unresolvable name. The teardown may be in there.
    Opaque,
}

/// The teardown bodies of an effect, at [`crate::ir::bindings::fn_binding_in`]'s
/// certainty bar: `return () => …`, and `return unsubscribe` where the name is
/// bound to exactly one literal in this body. Anything else is `Opaque`.
fn cleanup_bodies(body: &CFG) -> Cleanups<'_> {
    use crate::ir::expr::Prim;
    let mut bodies = Vec::new();
    let mut opaque = false;
    for block in body.blocks.values() {
        let Terminator::Return(expr) = &block.term else {
            continue;
        };
        match expr.peel_ts() {
            Expr::FnLit { body_cfg, .. } => bodies.push(&**body_cfg),
            Expr::Lit(Prim::Unit) => {}
            Expr::Var(v) => match crate::ir::bindings::fn_binding_in(v, body) {
                Some((_, cfg)) => bodies.push(cfg),
                None => opaque = true,
            },
            _ => opaque = true,
        }
    }
    match (bodies.is_empty(), opaque) {
        (_, true) => Cleanups::Opaque,
        (true, false) => Cleanups::None,
        (false, false) => Cleanups::Bodies(bodies),
    }
}

/// Does one of the cleanups undo this registration?
///
/// The listener must be the same **binding**: `removeEventListener('x', h)`
/// against `addEventListener('x', h)`. Matching on the teardown name alone
/// would certify exactly the shape `subscribe-with-fresh-listener` exists to
/// catch — a cleanup that removes a *different* listener.
fn pair(row: &Registration, cleanups: &Cleanups, fns: &HashMap<Var, Arc<CFG>>) -> Pairing {
    let Some(reg) = REGISTRARS.iter().find(|r| r.name == row.registrar) else {
        return Pairing::Unknown;
    };
    if reg.teardown.is_empty() {
        // Nothing can undo a promise continuation. That is a claim, not an
        // absence of evidence.
        return Pairing::Unpaired;
    }
    // An effect that returns nothing on any path takes nothing back, whatever
    // shape the listener has. This is decided before the listener is looked at
    // at all — it is the one case where the absence is total.
    let bodies = match cleanups {
        Cleanups::None => return Pairing::Unpaired,
        Cleanups::Opaque => return Pairing::Unknown,
        Cleanups::Bodies(b) => b,
    };
    let Expr::Var(listener) = row.callback.peel_ts() else {
        // An inline literal is a fresh listener no teardown can name, and an
        // unresolvable expression is one we cannot compare — with a cleanup
        // present, both leave the pairing unproven rather than refuted.
        return Pairing::Unknown;
    };
    if bodies
        .iter()
        .any(|c| tears_down(c, reg.teardown, listener, fns, 2))
    {
        Pairing::Paired
    } else {
        Pairing::Unpaired
    }
}

/// Is there a `teardown(…, listener, …)` call in this cleanup body?
fn tears_down(
    cfg: &CFG,
    names: &[&'static str],
    listener: &Var,
    fns: &HashMap<Var, Arc<CFG>>,
    depth: usize,
) -> bool {
    let mut hit = false;
    cfg.for_each_expr(&mut |e| hit = hit || teardown_in_expr(e, names, listener, fns, depth));
    hit
}

fn teardown_in_expr(
    expr: &Expr,
    names: &[&'static str],
    listener: &Var,
    fns: &HashMap<Var, Arc<CFG>>,
    depth: usize,
) -> bool {
    if let Expr::Call { fn_, args } = expr {
        let called = match fn_.peel_ts() {
            Expr::Var(n) => Some(n.as_str()),
            Expr::FieldAccess { field, .. } => Some(field.as_str()),
            _ => None,
        };
        if let Some(called) = called {
            if names.contains(&called)
                && args
                    .iter()
                    .any(|a| matches!(a.peel_ts(), Expr::Var(v) if v == listener))
            {
                return true;
            }
            // A teardown one local helper deep still tears down.
            if depth > 0
                && let Some(body) = fns.get(called)
                && tears_down(body, names, listener, fns, depth - 1)
            {
                return true;
            }
        }
    }
    let mut hit = false;
    expr.for_each_child(&mut |c| hit = hit || teardown_in_expr(c, names, listener, fns, depth));
    hit
}

// ── The scan ──────────────────────────────────────────────────────────────────

/// Scan a CFG for callback registrations. `fixed_block`:
/// - `None` → this IS the effect body; each statement carries its own block ID
///   (usable for must-reach);
/// - `Some(b)` → nested context (a helper called inline keeps the caller's
///   block, a callback body gets `Some(None)`).
fn scan_cfg(
    cfg: &CFG,
    fns: &HashMap<Var, Arc<CFG>>,
    depth: usize,
    fixed_block: Option<Option<BlockId>>,
    effect: HookLabel,
    out: &mut Vec<Registration>,
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
            scan_expr(expr, span, block_id, fns, depth, effect, out);
        }
        match &block.term {
            Terminator::Return(e) | Terminator::Branch { cond: e, .. } => {
                scan_expr(e, None, block_id, fns, depth, effect, out);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_expr(
    expr: &Expr,
    span: Option<SourceRange>,
    block_id: Option<BlockId>,
    fns: &HashMap<Var, Arc<CFG>>,
    depth: usize,
    effect: HookLabel,
    out: &mut Vec<Registration>,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Some((reg, display)) = match_registrar(fn_)
                && let Some(cb) = args.get(reg.cb_arg)
            {
                out.push(Registration {
                    effect,
                    display,
                    registrar: reg.name,
                    firing: reg.firing,
                    timing: reg.timing,
                    callback: cb.clone(),
                    block_id,
                    span,
                    pairing: Pairing::Unknown,
                });
            }
            // Direct call of a locally-bound helper executes inline: the
            // registration inside it happens on the caller's block (B6).
            if depth > 0
                && let Expr::Var(name) = fn_.peel_ts()
                && let Some(body) = fns.get(name)
            {
                scan_cfg(body, fns, depth - 1, Some(block_id), effect, out);
            }
            // Descend: chained receivers (`p.then(a).then(b)`), callback
            // bodies (a registration inside another callback is never
            // must-reached), plain args.
            scan_expr(fn_, span, block_id, fns, depth, effect, out);
            for arg in args {
                match arg {
                    Expr::FnLit { body_cfg, .. } if depth > 0 => {
                        scan_cfg(body_cfg, fns, depth - 1, Some(None), effect, out);
                    }
                    _ => scan_expr(arg, span, block_id, fns, depth, effect, out),
                }
            }
        }
        other => {
            other.for_each_child(&mut |c| scan_expr(c, span, block_id, fns, depth, effect, out));
        }
    }
}
