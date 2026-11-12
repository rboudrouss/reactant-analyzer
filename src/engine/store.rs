//! The store (#298, ADR-046): every library store and the URL together are
//! one program-wide pseudo-slot, [`STORE_SLOT`], owned by
//! [`ComponentId::STORE`] — a slot no component of the program owns. A whole
//! [`SummaryValue::Held`] hook result reads it (`render_deps::Source::Store`,
//! a versioned trigger row); a call that writes it is a may-write row of the
//! slot-writer relation with `owner: Some(ComponentId::STORE)`
//! (`setters::collect_slot_writers`), a foreign write the graph never kills.
//!
//! Which calls write the store is decided here and nowhere else. The writers
//! are the visible movers: a value a summary names a
//! [`SummaryValue::Navigator`], or such a member of a shaped hook result,
//! through the bindings; a bare `navigate` / `redirect`; a router, history,
//! navigation or location method that changes the URL, under `window` or
//! `document` too; a write of a `location` member; a `<Navigate/>` /
//! `<Redirect/>` element. A call the analysis cannot see into is assumed not
//! to write the store, the assumption the convergence proof already made of
//! it (#161, `docs/limitations.md`).
//!
//! The forms are read at two widths (`Forms`). A row claims a write, so it
//! reads `window` and `document` only through their `location`, `history` or
//! `navigation`: `document.title = …` moves no URL. The convergence proof's
//! veto keeps #161's reading, any listed method or member write under them:
//! narrowing it removes findings, and is measured on its own.

use std::collections::HashMap;

use crate::ir::{
    ComponentId, QualifiedSlot,
    bindings::local_bindings,
    cfg::CFG,
    expr::{Expr, MarkerVal, Prim, SummaryValue},
    stmt::{MemberKey, Stmt},
};

/// The store: one slot for every library store and the URL.
pub const STORE_SLOT: QualifiedSlot = (ComponentId::STORE, 0);

/// How wide the syntactic forms are read (module doc).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Forms {
    /// What a row claims: `window` and `document` only through their
    /// `location`, `history` or `navigation`.
    Row,
    /// #161's veto: anything listed under `window` or `document`.
    Veto,
}

/// The names whose listed methods change the URL.
const ROUTERS: &[&str] = &["router", "history", "navigation", "location"];
/// The globals that carry them (`window.history.pushState(…)`).
const GLOBALS: &[&str] = &["window", "document"];
const METHODS: &[&str] = &[
    "push",
    "replace",
    "navigate",
    "assign",
    "reload",
    "pushState",
    "replaceState",
    "back",
    "forward",
    "go",
];
const BARE: &[&str] = &["navigate", "redirect"];
const ELEMENTS: &[&str] = &["Navigate", "Redirect"];

/// What a name may be bound to where a call is read: every right-hand side
/// the caller's scopes give it.
pub(crate) type Bound<'e, 'b> = &'b dyn Fn(&str) -> Vec<&'e Expr>;

/// The first member read off the root name of the path `e`
/// (`window.location.href` → `location`).
fn first_member(e: &Expr) -> Option<&str> {
    match e.peel_ts() {
        Expr::FieldAccess { obj, field } => match obj.peel_ts() {
            Expr::Var(_) => Some(field.as_str()),
            _ => first_member(obj),
        },
        Expr::IndexAccess { arr, .. } => first_member(arr),
        _ => None,
    }
}

/// Does calling `fn_` write the store?
pub(crate) fn call_writes<'e>(fn_: &'e Expr, bound: Bound<'e, '_>, forms: Forms) -> bool {
    if mover(fn_, bound, 8) {
        return true;
    }
    match fn_.peel_ts() {
        Expr::Var(v) => BARE.contains(&v.as_str()),
        Expr::FieldAccess { obj, field } => {
            METHODS.contains(&field.as_str())
                && obj.root_var().is_some_and(|r| {
                    ROUTERS.contains(&r.as_str())
                        || (GLOBALS.contains(&r.as_str())
                            && (forms == Forms::Veto
                                || first_member(obj).is_some_and(|m| ROUTERS.contains(&m))))
                })
        }
        _ => false,
    }
}

/// Does rendering the element `name` write the store?
pub(crate) fn element_writes(name: &str) -> bool {
    ELEMENTS.contains(&name)
}

/// Does the member write `obj.key = …` write the store: is it a write into
/// `location`?
pub(crate) fn member_write_writes(obj: &Expr, key: &MemberKey, forms: Forms) -> bool {
    obj.root_var().is_some_and(|r| {
        r.as_str() == "location"
            || (GLOBALS.contains(&r.as_str())
                && (forms == Forms::Veto
                    || first_member(obj).is_some_and(|m| m == "location")
                    || matches!((obj.peel_ts(), key), (Expr::Var(_), MemberKey::Field(f))
                        if f.as_str() == "location")))
    })
}

/// Is `e` a spelling of a mover, through the bindings?
fn mover<'e>(e: &'e Expr, bound: Bound<'e, '_>, depth: usize) -> bool {
    if depth == 0 {
        return false;
    }
    match e.peel_ts() {
        Expr::HookMarker(_, MarkerVal::Summary(sv)) | Expr::SummaryVal(sv) => {
            matches!(sv, SummaryValue::Navigator { .. })
        }
        Expr::Var(v) => bound(v).into_iter().any(|r| mover(r, bound, depth - 1)),
        Expr::FieldAccess { obj, field } => matches!(
            shape_member(obj, field, bound, depth - 1),
            Some(SummaryValue::Navigator { .. })
        ),
        Expr::IndexAccess { arr, idx } => matches!(idx.peel_ts(), Expr::Lit(Prim::Int(i))
            if matches!(shape_member(arr, &i.to_string(), bound, depth - 1),
                Some(SummaryValue::Navigator { .. }))),
        _ => false,
    }
}

/// The summary of member `field` of the shaped hook result `e` names.
fn shape_member<'e>(
    e: &'e Expr,
    field: &str,
    bound: Bound<'e, '_>,
    depth: usize,
) -> Option<&'e SummaryValue> {
    if depth == 0 {
        return None;
    }
    match e.peel_ts() {
        Expr::HookMarker(_, MarkerVal::Summary(SummaryValue::Shape { members, .. })) => {
            members.iter().find(|(k, _)| k == field).map(|(_, v)| v)
        }
        Expr::Var(v) => bound(v)
            .into_iter()
            .find_map(|r| shape_member(r, field, bound, depth - 1)),
        _ => None,
    }
}

/// Does `body` write the store anywhere — nested closures included, whether
/// or not anything calls them: a redirect sits in a continuation as often as
/// in the body? A name resolves in the closure that binds it, else in
/// `render`, where a mover is bound and called from a body. This is the
/// program-wide reading the convergence proof takes (`churn::build_edges`),
/// beside the rows the writer walk places.
pub(crate) fn body_writes<'e>(body: &'e CFG, render: &HashMap<&'e str, Vec<&'e Expr>>) -> bool {
    let local = local_bindings(body);
    let bound = |v: &str| -> Vec<&'e Expr> {
        local
            .get(v)
            .or_else(|| render.get(v))
            .cloned()
            .unwrap_or_default()
    };
    fn expr<'e>(
        e: &'e Expr,
        bound: Bound<'e, '_>,
        render: &HashMap<&'e str, Vec<&'e Expr>>,
    ) -> bool {
        match e {
            Expr::Call { fn_, .. } if call_writes(fn_, bound, Forms::Veto) => return true,
            Expr::CompApp { name, .. } if element_writes(name) => return true,
            Expr::FnLit { body_cfg, .. } => return body_writes(body_cfg, render),
            _ => {}
        }
        let mut found = false;
        e.for_each_child(&mut |c| found |= expr(c, bound, render));
        found
    }
    let mut found = false;
    body.for_each_expr(&mut |e| found |= expr(e, &bound, render));
    found
        || body.blocks.values().any(|b| {
            b.stmts.iter().any(|s| {
                matches!(s, Stmt::MemberWrite { obj, key, .. }
                    if member_write_writes(obj, key, Forms::Veto))
            })
        })
}
