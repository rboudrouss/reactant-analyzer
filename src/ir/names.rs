//! How a variable of the IR is named to the user: the source expression it
//! stands for, never the spelling lowering gave it.
//!
//! A `Var` is an identity, not a display name. Three things in it are not
//! source text: the `#salt` an inlined hook's locals carry, the `.1` of an
//! index segment (`t[1]` is the path `t.1`, which is what the converged-value
//! tables key a member call by), and a lowering temp (`__call_207` for the
//! value `get()` returns, `__hook_0` for what `useThing()` returns). The first
//! two are syntax, read off the name by [`source_name`]; the third is a
//! binding, read off the temp's `Let` by [`SourceNames`].

use std::borrow::Cow;
use std::collections::HashMap;

use crate::ir::{
    bindings::nested_bodies, cfg::CFG, expr::Expr, hooks::HookProvenance, splice::SPLICE_MARK,
    stmt::Stmt, types::Var,
};

/// Separates the segments of a member path (`api.setV`, `t.1`).
const PATH_MARK: char = '.';

/// The name a variable or member path has in the source, for display: the
/// rename salt of a spliced local is dropped from the root (`setC#0` →
/// `setC`, `t#3.1` → `t[1]`) and an index segment reads as an index
/// (`t.1` → `t[1]`). Never use this for matching — only the full name is
/// unique.
pub fn source_name(var: &str) -> Cow<'_, str> {
    let (root, path) = match var.find(PATH_MARK) {
        Some(at) => var.split_at(at),
        None => (var, ""),
    };
    let root = root.split_once(SPLICE_MARK).map_or(root, |(base, _)| base);
    if path.is_empty() {
        return Cow::Borrowed(root);
    }
    let mut out = String::from(root);
    push_path(&mut out, path);
    Cow::Owned(out)
}

/// Append a path's segments (`.setV`, `.1`) as source writes them.
fn push_path(out: &mut String, path: &str) {
    for seg in path.split(PATH_MARK).skip(1) {
        if !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
            out.push('[');
            out.push_str(seg);
            out.push(']');
        } else {
            out.push(PATH_MARK);
            out.push_str(seg);
        }
    }
}

/// The lowering temps that stand for a call's value, with the callee of that
/// call as the source wrote it. Built once per body from the temps' own
/// `Let`s, so a temp an inlined hook carried in (renamed, `__hook_0#2`) is
/// found where the splice put it.
#[derive(Debug, Default)]
pub struct SourceNames {
    callees: HashMap<Var, Expr>,
}

/// Lowering names the value of a callee that is itself a call `__call_N`
/// (`get()(x)`, #46), and the value of a hook call staged out of an
/// expression `__hook_N` (`useThing().set(x)`, #233).
fn is_call_temp(var: &str) -> bool {
    var.starts_with("__call_") || var.starts_with("__hook_")
}

/// Bound on temps read through temps (`get()()()`): lowering never makes a
/// cycle, the bound only keeps a malformed IR from looping.
const MAX_TEMP_DEPTH: usize = 8;

impl SourceNames {
    /// The call temps bound anywhere in `cfg`, its function literals included.
    /// `provenance` names a hook marker's callee. A call inlined into `cfg`
    /// left no call behind, only the variable the splice bound its return to:
    /// `inlined` lists those, as (variable, callee).
    pub fn of<'a>(
        cfg: &CFG,
        provenance: &[HookProvenance],
        inlined: impl IntoIterator<Item = (&'a Var, &'a str)>,
    ) -> Self {
        let mut callees = HashMap::new();
        for (var, callee) in inlined {
            if is_call_temp(var) {
                callees.insert(var.clone(), Expr::Var(callee.to_string()));
            }
        }
        let bodies = std::iter::once(cfg).chain(nested_bodies(cfg).into_iter().map(|(_, b)| b));
        for body in bodies {
            for block in body.blocks.values() {
                for stmt in &block.stmts {
                    let Stmt::Let { var, rhs, .. } = stmt else {
                        continue;
                    };
                    if !is_call_temp(var) {
                        continue;
                    }
                    let callee = match rhs.peel_ts() {
                        Expr::HookMarker(label, _) => provenance
                            .iter()
                            .find(|p| p.label == *label)
                            .map(|p| Expr::Var(p.origin_hook.clone())),
                        Expr::Call { fn_, .. } => Some((**fn_).clone()),
                        _ => None,
                    };
                    if let Some(callee) = callee {
                        callees.insert(var.clone(), callee);
                    }
                }
            }
        }
        SourceNames { callees }
    }

    /// [`source_name`], with a call temp at the root read as the call it binds:
    /// `__call_207` → `get(…)`, `__hook_0.set` → `useThing(…).set`, and a temp
    /// whose callee is itself a temp (`get()()`) → `get(…)(…)`.
    pub fn name<'a>(&'a self, var: &'a str) -> Cow<'a, str> {
        self.name_at(var, 0)
    }

    fn name_at<'a>(&'a self, var: &'a str, depth: usize) -> Cow<'a, str> {
        let (root, path) = match var.find(PATH_MARK) {
            Some(at) => var.split_at(at),
            None => (var, ""),
        };
        let Some(callee) = self.callees.get(root).filter(|_| depth < MAX_TEMP_DEPTH) else {
            return source_name(var);
        };
        let mut out = format!("{}(…)", self.expr_at(callee, depth + 1));
        push_path(&mut out, path);
        Cow::Owned(out)
    }

    /// A callee as the source wrote it: [`Expr::describe`], with every
    /// variable on its spine read through this table.
    fn expr_at(&self, e: &Expr, depth: usize) -> String {
        match e.peel_ts() {
            Expr::Var(v) => self.name_at(v, depth).into_owned(),
            Expr::FieldAccess { obj, field } => format!("{}.{field}", self.expr_at(obj, depth)),
            Expr::IndexAccess { arr, .. } => format!("{}[…]", self.expr_at(arr, depth)),
            Expr::Call { fn_, .. } => format!("{}(…)", self.expr_at(fn_, depth)),
            other => other.describe(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_salt_is_dropped_from_the_root_only() {
        assert_eq!(source_name("count"), "count");
        assert_eq!(source_name("count#3"), "count");
        assert_eq!(source_name("api#3.setV"), "api.setV");
        assert_eq!(source_name("this.#field"), "this.#field");
    }

    #[test]
    fn an_index_segment_reads_as_an_index() {
        assert_eq!(source_name("t.1"), "t[1]");
        assert_eq!(source_name("t#3.1"), "t[1]");
        assert_eq!(source_name("state.rows.0.set"), "state.rows[0].set");
    }

    #[test]
    fn a_call_temp_reads_as_its_call() {
        let names = SourceNames::of(
            &CFG {
                entry: 0,
                blocks: Default::default(),
                edges: vec![],
            },
            &[],
            [(&"__call_207".to_string(), "get")],
        );
        assert_eq!(names.name("__call_207"), "get(…)");
        assert_eq!(names.name("__call_207.1"), "get(…)[1]");
        assert_eq!(names.name("t.1"), "t[1]");
    }

    #[test]
    fn a_temp_whose_callee_is_a_temp_reads_through_it() {
        // `get()()(x)`: the inner call is inlined (no call left), the outer
        // one is a `Let` of a call whose callee is the inner temp.
        let outer = Stmt::Let {
            var: "__call_9".to_string(),
            rhs: Expr::Call {
                fn_: Box::new(Expr::Var("__call_5".to_string())),
                args: vec![],
            },
            span: None,
        };
        let names = SourceNames::of(
            &crate::test_support::single_block_cfg(vec![outer]),
            &[],
            [(&"__call_5".to_string(), "get")],
        );
        assert_eq!(names.name("__call_9"), "get(…)(…)");
    }
}
