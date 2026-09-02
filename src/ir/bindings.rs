//! Syntactic binding certificates: what a name is bound to, and whether that
//! binding is *certain*.
//!
//! Resolving `useStore(sel)` to the body of `const sel = s => …` is only sound
//! when the name cannot mean something else at the call. The two levels here
//! answer that at two strengths, and consumers pick the one their claim needs:
//!
//! - [`closure_binding_of`] — one binding, a closure, within **one** body,
//!   naming which of its two spellings it is. The `missing-deps` /
//!   `stale-closure` bar, and the only reader that takes a *path*: a closure a
//!   custom hook handed back inside a container is the same binding one hop in.
//! - [`fn_binding_in`] — that reader narrowed to a bare name and to the
//!   literal spelling, for consumers that only handle one.
//! - [`certified_fn_binding`] — the same, plus **no rebinding anywhere below**:
//!   no `Let` and no `Assign` of that name in any nested function body. What a
//!   consumer needs before it may *execute* the body and keep the result.
//!
//! Both fail closed. An unproven name is not "not a function" — it is a name
//! whose meaning we decline to fix, so callers answer ⊤ rather than guess.

use std::collections::HashMap;

use crate::ir::{
    cfg::CFG,
    expr::{Expr, object_member},
    stmt::Stmt,
    types::{HookLabel, Var},
};

/// Every `let`/assignment right-hand side in `cfg`, by name — the bindings a
/// chase can follow. Nested `FnLit` bodies are deliberately not descended: a
/// name bound inside a closure is that closure's local, not this body's.
///
/// The weakest of the three readings here, and the only one that keeps ALL of
/// a name's right-hand sides: a consumer that needs certainty asks how many
/// there are, one that only needs "could this be X" scans them all.
pub fn local_bindings(cfg: &CFG) -> HashMap<&str, Vec<&Expr>> {
    let mut map: HashMap<&str, Vec<&Expr>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                map.entry(var.as_str()).or_default().push(rhs);
            }
        }
    }
    map
}

/// The two spellings a function-valued binding can have. Hook extraction
/// rewrites `useCallback(fn, deps)` to `CallbackVal(label)` and lifts `fn` out
/// of the render CFG, so the second spelling answers a label and a consumer
/// that wants the body asks the hook table for it.
pub enum ClosureBinding<'c> {
    Lit { params: &'c [Var], body: &'c CFG },
    Callback(HookLabel),
}

/// The unique closure the path `root.segments…` names in `cfg`, if any.
///
/// A bare name is the base case; each segment steps into the field of the sole
/// `ObjectLit` the prefix is bound to. That step is the whole point: a custom
/// hook that returns `{ clearFieldError }` hands its caller exactly the closure
/// a bare `const clearFieldError = useCallback(…)` would have, and the question
/// asked of it is the same one.
///
/// Conditional or repeated re-binding bails out (`None`) at every hop: the
/// captured environment is no longer syntactically certain. Nested bodies are
/// NOT scanned — see [`certified_fn_binding`] for the stronger reading.
pub fn closure_binding_of<'c>(
    root: &str,
    segments: &[String],
    cfg: &'c CFG,
) -> Option<ClosureBinding<'c>> {
    let mut cur = chase_var(root, cfg)?;
    for seg in segments {
        let Expr::ObjectLit { fields, .. } = cur else {
            return None;
        };
        cur = match object_member(fields, seg)?.peel_ts() {
            Expr::Var(v) => chase_var(v, cfg)?,
            e => e,
        };
    }
    match cur {
        Expr::FnLit {
            params, body_cfg, ..
        } => Some(ClosureBinding::Lit {
            params,
            body: body_cfg,
        }),
        Expr::CallbackVal(l) => Some(ClosureBinding::Callback(*l)),
        _ => None,
    }
}

/// The params and body of the unique `FnLit` bound to `var` in `cfg`, if any —
/// [`closure_binding_of`] for a bare name, narrowed to the literal spelling.
pub fn fn_binding_in<'c>(var: &str, cfg: &'c CFG) -> Option<(&'c [Var], &'c CFG)> {
    match closure_binding_of(var, &[], cfg)? {
        ClosureBinding::Lit { params, body } => Some((params, body)),
        ClosureBinding::Callback(_) => None,
    }
}

/// [`sole_binding_in`] followed through aliases: `{ bump }` records the member
/// as `Var("bump")`, so a chase that stopped at the first right-hand side would
/// see a name where the value is. The same propagation the interpreter does
/// when it binds a right-hand side, and bounded because a certain binding chain
/// is finite anyway.
fn chase_var<'c>(var: &str, cfg: &'c CFG) -> Option<&'c Expr> {
    let mut cur = sole_binding_in(var, cfg)?;
    for _ in 0..MAX_ALIAS_HOPS {
        let Expr::Var(next) = cur else {
            return Some(cur);
        };
        cur = sole_binding_in(next, cfg)?;
    }
    None
}

/// Alias hops a chase will follow before giving up. A real chain is one or two
/// (`{ bump }`, a destructuring preamble); the bound only stops a cycle.
const MAX_ALIAS_HOPS: usize = 8;

/// The single right-hand side `var` is bound to in `cfg`, `None` when it is
/// bound zero times or more than once. Conditional or repeated re-binding is
/// not "some value we haven't found" — it is a name whose captured environment
/// is no longer syntactically certain, so the readers above fail closed.
fn sole_binding_in<'c>(var: &str, cfg: &'c CFG) -> Option<&'c Expr> {
    let mut found: Option<&Expr> = None;
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            let (Stmt::Let { var: v, rhs, .. } | Stmt::Assign { var: v, rhs, .. }) = stmt else {
                continue;
            };
            if v != var {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(rhs.peel_ts());
        }
    }
    found
}

/// [`fn_binding_in`], certified against every nested function body: the name
/// must be bound exactly once, to a function literal, and never re-bound or
/// assigned inside any closure below `root`.
///
/// That last clause is what makes the binding usable at an arbitrary later
/// program point. A handler or effect that reassigns the name
/// (`useEffect(() => { sel = other })`) means the value flowing into a call is
/// not the literal this returns, and no order of evaluation makes it so — so
/// the certificate is refused rather than qualified.
pub fn certified_fn_binding<'c>(
    var: &str,
    root: &'c CFG,
    also: &[&'c CFG],
) -> Option<(&'c [Var], &'c CFG)> {
    let binding = fn_binding_in(var, root)?;
    if bound_below(var, root) {
        return None;
    }
    // Hook extraction lifts effect/callback/memo bodies OUT of the render CFG,
    // so they are not reachable by descending it — they must be handed in, or
    // a `useEffect(() => { sel = other })` rebinding would be invisible and the
    // certificate would certify a body the call never receives.
    for body in also {
        if binds(var, body) || bound_below(var, body) {
            return None;
        }
    }
    Some(binding)
}

/// Is `var` bound or assigned anywhere in a function body nested under `cfg`?
fn bound_below(var: &str, cfg: &CFG) -> bool {
    let mut hit = false;
    for_each_nested_body(cfg, &mut |body| {
        hit = hit || binds(var, body) || bound_below(var, body);
    });
    hit
}

/// Does this body bind or assign `var` at its own level?
fn binds(var: &str, cfg: &CFG) -> bool {
    cfg.blocks.values().any(|b| {
        b.stmts.iter().any(|s| {
            matches!(
                s,
                Stmt::Let { var: v, .. } | Stmt::Assign { var: v, .. } if v == var
            )
        })
    })
}

/// Call `f` for every function body one level down from `cfg`.
fn for_each_nested_body<'c>(cfg: &'c CFG, f: &mut impl FnMut(&'c CFG)) {
    cfg.for_each_expr(&mut |e| each_fn_lit(e, f));
}

fn each_fn_lit<'c>(e: &'c Expr, f: &mut impl FnMut(&'c CFG)) {
    if let Expr::FnLit { body_cfg, .. } = e {
        f(body_cfg);
    }
    e.for_each_child(&mut |c| each_fn_lit(c, f));
}
