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
    types::{BlockId, HookLabel, Var},
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
    let mut cur = binding_of(root, cfg)?;
    for seg in segments {
        let Expr::ObjectLit { fields, .. } = cur else {
            return None;
        };
        cur = match object_member(fields, seg)?.peel_ts() {
            Expr::Var(v) => binding_of(v, cfg)?,
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

/// The value `var` names in `cfg`: its sole right-hand side, followed through
/// any chain of renames. `{ bump }` records the member as `Var("bump")`, so a
/// chase that stopped at the first right-hand side would see a name where the
/// value is. The same propagation the interpreter does when it binds a
/// right-hand side, and bounded because a certain binding chain is finite
/// anyway.
pub fn binding_of<'c>(var: &str, cfg: &'c CFG) -> Option<&'c Expr> {
    binding_site_of(var, cfg).map(|(_, rhs)| rhs)
}

/// [`binding_of`], with the block of `cfg` the value is bound in — where the
/// chase ended, not where `var` itself is bound. A closure is authored where
/// its literal sits, whatever names it is later read through.
pub fn binding_site_of<'c>(var: &str, cfg: &'c CFG) -> Option<(BlockId, &'c Expr)> {
    let mut cur = sole_binding_in(var, cfg)?;
    for _ in 0..MAX_ALIAS_HOPS {
        let Expr::Var(next) = cur.1 else { break };
        let Some(deeper) = sole_binding_in(next, cfg) else {
            break;
        };
        cur = deeper;
    }
    Some(cur)
}

/// The state slot `expr` reads on every path: `StateVal(l)` itself, or a name
/// whose sole binding in `scope`, through renames that are sole bindings too,
/// is `StateVal(l)`. A must-alias, where `resolve_setter_aliases` keeps every
/// name *some* binding ties to a slot (#210). `elsewhere` lists every other
/// body a read of the name may sit in or be reached from, each with its
/// parameters (nested ones included: see [`nested_bodies`]): a binder of a
/// chain name in one of them (a reassignment, a same-named local or
/// parameter) refuses the certificate, since the name a read there sees may
/// be another value.
pub fn must_slot_read(expr: &Expr, scope: &CFG, elsewhere: &[(&[Var], &CFG)]) -> Option<HookLabel> {
    match must_binding(expr, scope, elsewhere)? {
        Expr::StateVal(l) => Some(*l),
        _ => None,
    }
}

/// The parameter of `params` that `expr` denotes on every path: itself, or
/// a name whose chain of sole bindings in `scope` ends at it — the `deps` of
/// a `deps = []` parameter, which lowers to `let deps = __p1` (#230). No
/// body binds a chain name again: not `scope` (the parameter itself
/// included), not `elsewhere`, under the certificate [`must_slot_read`]
/// states.
pub fn must_param(
    expr: &Expr,
    scope: &CFG,
    elsewhere: &[(&[Var], &CFG)],
    params: &[Var],
) -> Option<usize> {
    let mut cur = expr.peel_ts();
    for _ in 0..MAX_ALIAS_HOPS + 2 {
        let Expr::Var(v) = cur else {
            return None;
        };
        if elsewhere
            .iter()
            .any(|(ps, body)| ps.contains(v) || binds(v, body))
        {
            return None;
        }
        if let Some(i) = params.iter().position(|p| p == v) {
            return (!binds(v, scope)).then_some(i);
        }
        cur = sole_binding_in(v, scope)?.1;
    }
    None
}

/// [`must_slot_read`] through member reads: the slot `expr` reads on every
/// path and the fields it reads off it — `s.n`, or the `n` of a `const { n }
/// = s` (#244). An empty path is the slot itself.
pub fn must_slot_path(
    expr: &Expr,
    scope: &CFG,
    elsewhere: &[(&[Var], &CFG)],
) -> Option<(HookLabel, Vec<String>)> {
    fn go(
        expr: &Expr,
        scope: &CFG,
        elsewhere: &[(&[Var], &CFG)],
        depth: usize,
    ) -> Option<(HookLabel, Vec<String>)> {
        match must_binding(expr, scope, elsewhere)? {
            Expr::StateVal(l) => Some((*l, Vec::new())),
            Expr::FieldAccess { obj, field } if depth > 0 => {
                let (l, mut path) = go(obj, scope, elsewhere, depth - 1)?;
                path.push(field.clone());
                Some((l, path))
            }
            _ => None,
        }
    }
    go(expr, scope, elsewhere, MAX_ALIAS_HOPS)
}

/// The value `expr` denotes on every path: itself when it is no name, or
/// the end of the chain of sole bindings in `scope` a name starts, under the
/// certificate [`must_slot_read`] states — no binder of a chain name in any
/// body of `elsewhere`. What a name is *certainly* bound to, whatever it is
/// (a slot's read, a setter: #235).
pub fn must_binding<'e>(
    expr: &'e Expr,
    scope: &'e CFG,
    elsewhere: &[(&[Var], &CFG)],
) -> Option<&'e Expr> {
    let mut cur = expr.peel_ts();
    // The name's own binding, then the hops `binding_of` follows.
    for _ in 0..MAX_ALIAS_HOPS + 2 {
        match cur {
            Expr::Var(v) => {
                if elsewhere
                    .iter()
                    .any(|(params, body)| params.contains(v) || binds(v, body))
                {
                    return None;
                }
                cur = sole_binding_in(v, scope)?.1;
            }
            other => return Some(other),
        }
    }
    None
}

/// [`must_binding`] through members: `api.inc` is the member `inc` of the
/// object literal `api` is certainly bound to (the readable one, after every
/// spread), `t[1]` the element of an exact array literal — when nothing can
/// have replaced it since: no body, `scope` or `elsewhere`, uses the root
/// other than to read a member (no member write rooted at it, no mutating
/// method, no alias, no argument).
pub fn must_member_binding<'e>(
    expr: &'e Expr,
    scope: &'e CFG,
    elsewhere: &[(&[Var], &CFG)],
) -> Option<&'e Expr> {
    let (obj, key) = match expr.peel_ts() {
        Expr::FieldAccess { obj, field } => (obj, field.clone()),
        Expr::IndexAccess { arr, idx } => match idx.peel_ts() {
            Expr::Lit(crate::ir::Prim::Int(i)) if *i >= 0 => (arr, i.to_string()),
            _ => return None,
        },
        _ => return must_binding(expr, scope, elsewhere),
    };
    let root = root_var(obj)?;
    if std::iter::once(scope)
        .chain(elsewhere.iter().map(|(_, body)| *body))
        .any(|body| used_whole(root, body))
    {
        return None;
    }
    let member = match must_member_binding(obj, scope, elsewhere)? {
        Expr::ObjectLit { fields, .. } => object_member(fields, &key)?,
        Expr::ArrayLit {
            elems,
            arity,
            spread_at,
            ..
        } if spread_at.is_empty() && arity.exact() == Some(elems.len()) => {
            elems.get(key.parse::<usize>().ok()?)?
        }
        _ => return None,
    };
    must_member_binding(member, scope, elsewhere)
}

/// The variable a member chain is rooted at: `api` for `api.a[0]`.
fn root_var(expr: &Expr) -> Option<&Var> {
    match expr.peel_ts() {
        Expr::Var(v) => Some(v),
        Expr::FieldAccess { obj, .. } => root_var(obj),
        Expr::IndexAccess { arr, .. } => root_var(arr),
        _ => None,
    }
}

/// Does `cfg`, closures included, use `root` otherwise than as the receiver
/// of a member read — the uses through which a member may be replaced?
fn used_whole(root: &str, cfg: &CFG) -> bool {
    fn expr(root: &str, e: &Expr) -> bool {
        if crate::ir::expr::mutation_receiver(e)
            .and_then(root_var)
            .is_some_and(|r| r == root)
        {
            return true;
        }
        match e.peel_ts() {
            Expr::Var(v) => v == root,
            Expr::FieldAccess { obj, .. } if matches!(obj.peel_ts(), Expr::Var(v) if v == root) => {
                false
            }
            Expr::IndexAccess { arr, idx } if matches!(arr.peel_ts(), Expr::Var(v) if v == root) => {
                expr(root, idx)
            }
            Expr::FnLit {
                params, body_cfg, ..
            } => !params.iter().any(|p| p == root) && used_whole(root, body_cfg),
            other => {
                let mut found = false;
                other.for_each_child(&mut |c| found = found || expr(root, c));
                found
            }
        }
    }
    cfg.blocks.values().any(|block| {
        let written = block.stmts.iter().any(|s| {
            matches!(s, Stmt::MemberWrite { obj, .. } if root_var(obj).is_some_and(|r| r == root))
        });
        let mut found = written;
        block.for_each_expr(&mut |e| found = found || expr(root, e));
        found
    })
}

/// Every function body nested under `cfg`, at any depth, with its parameters.
pub fn nested_bodies(cfg: &CFG) -> Vec<(&[Var], &CFG)> {
    fn walk<'c>(e: &'c Expr, out: &mut Vec<(&'c [Var], &'c CFG)>) {
        if let Expr::FnLit {
            params, body_cfg, ..
        } = e
        {
            out.push((params, body_cfg));
            body_cfg.for_each_expr(&mut |c| walk(c, out));
            return;
        }
        e.for_each_child(&mut |c| walk(c, out));
    }
    let mut out = Vec::new();
    cfg.for_each_expr(&mut |e| walk(e, &mut out));
    out
}

/// Alias hops a chase will follow. A real chain is one or two (`{ bump }`, a
/// destructuring preamble); the bound only stops a cycle, and stopping leaves
/// the name itself — which no caller mistakes for a value.
pub const MAX_ALIAS_HOPS: usize = 8;

/// The single right-hand side `var` is bound to in `cfg`, and its block;
/// `None` when it is bound zero times or more than once. Conditional or
/// repeated re-binding is not "some value we haven't found" — it is a name
/// whose captured environment is no longer syntactically certain, so the
/// readers above fail closed.
fn sole_binding_in<'c>(var: &str, cfg: &'c CFG) -> Option<(BlockId, &'c Expr)> {
    let mut found: Option<(BlockId, &Expr)> = None;
    for (&bid, block) in &cfg.blocks {
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
            found = Some((bid, rhs.peel_ts()));
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
