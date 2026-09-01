//! Syntactic binding certificates: what a name is bound to, and whether that
//! binding is *certain*.
//!
//! Resolving `useStore(sel)` to the body of `const sel = s => …` is only sound
//! when the name cannot mean something else at the call. The two levels here
//! answer that at two strengths, and consumers pick the one their claim needs:
//!
//! - [`fn_binding_in`] — one binding, a function literal, within **one** body.
//!   The `missing-deps` / `stale-closure` bar: enough to read a callback whose
//!   binding sits beside its registration.
//! - [`certified_fn_binding`] — the same, plus **no rebinding anywhere below**:
//!   no `Let` and no `Assign` of that name in any nested function body. What a
//!   consumer needs before it may *execute* the body and keep the result.
//!
//! Both fail closed. An unproven name is not "not a function" — it is a name
//! whose meaning we decline to fix, so callers answer ⊤ rather than guess.

use crate::ir::{cfg::CFG, expr::Expr, stmt::Stmt, types::Var};

/// The params and body of the unique `FnLit` bound to `var` in `cfg`, if any.
/// Conditional or repeated re-binding bails out (`None`): the captured
/// environment is no longer syntactically certain. Nested bodies are NOT
/// scanned — see [`certified_fn_binding`] for the stronger reading.
pub fn fn_binding_in<'c>(var: &str, cfg: &'c CFG) -> Option<(&'c [Var], &'c CFG)> {
    let mut found: Option<(&[Var], &CFG)> = None;
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            let (Stmt::Let { var: v, rhs, .. } | Stmt::Assign { var: v, rhs, .. }) = stmt else {
                continue;
            };
            if v != var {
                continue;
            }
            match rhs.peel_ts() {
                Expr::FnLit {
                    params, body_cfg, ..
                } if found.is_none() => found = Some((params, body_cfg)),
                _ => return None,
            }
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
