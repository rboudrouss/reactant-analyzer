//! How a variable of the IR is named to the user: the source expression it
//! stands for, never the spelling lowering gave it.
//!
//! A `Var` is an identity, not a display name. Three things in it are not
//! source text: the `#salt` an inlined hook's locals carry, the `.1` of an
//! index segment (`t[1]` is the path `t.1`, which is what the converged-value
//! tables key a member call by), and a lowering temp (`__call_207` for the
//! value `get()` returns, `__hook_0` for what `useThing()` returns, `__t12_30`
//! for the value of `a ?? b`). The first two are syntax, read off the name by
//! [`source_name`]; the third is a binding, read off the temp's `Let` by
//! [`SourceNames`]: its callee, or the source text the lowering recorded for
//! it in the [`FileTable`]. Every message names a variable or an expression
//! through [`SourceNames`].

use std::borrow::Cow;
use std::collections::HashMap;

use crate::ir::{
    FileTable,
    bindings::{element_binding, nested_bodies},
    cfg::CFG,
    expr::Expr,
    hooks::{HookEntry, HookProvenance},
    splice::SPLICE_MARK,
    stmt::Stmt,
    types::{HookLabel, Var},
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
/// call as the source wrote it, and the temps that stand for any other
/// expression's value, with its source text. Built once per body from the
/// temps' own `Let`s, so a temp an inlined hook carried in (renamed,
/// `__hook_0#2`) is found where the splice put it.
#[derive(Debug, Default)]
pub struct SourceNames {
    callees: HashMap<Var, Expr>,
    /// A temp the lowering minted for an expression's value (`a ?? b`,
    /// `c ? x : y`, `a?.f()`), read in the [`FileTable`] of the file its
    /// `Let` sits in.
    texts: HashMap<Var, String>,
    /// The name the render's `useState` destructuring declares for each slot
    /// (`const [n, setN]`): a `Let` of the slot's read the component itself
    /// writes, not an inlined hook, the smallest such name if there are
    /// several (#278); else a member of a stored pair (`t[0]`), or a name the
    /// slot reaches through a custom hook's result (#288).
    slots: HashMap<HookLabel, Var>,
}

/// Lowering names the value of a callee that is itself a call `__call_N`
/// (`get()(x)`, #46), and the value of a hook call staged out of an
/// expression `__hook_N` (`useThing().set(x)`, #233).
fn is_call_temp(var: &str) -> bool {
    var.starts_with("__call_") || var.starts_with("__hook_")
}

/// The temp an inlined body's argument no parameter takes is bound to,
/// `__argN_K` (`ir::splice`, #324): it reads as the argument.
pub(crate) fn is_argument_temp(var: &str) -> bool {
    var.starts_with("__arg")
}

/// The prefixes of the names lowering mints: an expression's value (`__t`,
/// `__sw`, `__of`), a call's (`__call_`, `__hook_`, `__hookarg_`), a
/// destructured value, element or default (`__dstr_`, `__arr_`, `__obj_`,
/// `__dflt_`), a destructured parameter (`__p`), an inlined body's argument
/// (`__arg`).
const TEMP_PREFIXES: &[&str] = &[
    "__t",
    "__sw",
    "__of",
    "__call_",
    "__hookarg_",
    "__hook_",
    "__dstr_",
    "__arr_",
    "__obj_",
    "__dflt_",
    "__p",
    "__arg",
];

/// `true` for a name lowering minted, whatever salt or member path follows
/// it: a prefix lowering mints (`TEMP_PREFIXES`) then digits (`__t12_30`, `__p0.a`,
/// `__hook_1#2`). A source name may start with `__` too (`__DEV__`), so the
/// shape is checked whole. No message prints such a name.
pub fn is_lowering_temp(var: &str) -> bool {
    let root = var.split([PATH_MARK, SPLICE_MARK]).next().unwrap_or(var);
    TEMP_PREFIXES.iter().any(|p| {
        root.strip_prefix(p).is_some_and(|rest| {
            rest.starts_with(|c: char| c.is_ascii_digit())
                && rest.bytes().all(|b| b.is_ascii_digit() || b == b'_')
        })
    })
}

/// The temp a destructuring default binds, `__dflt_N` (#256): written once
/// with the member the pattern reads, then again with the default when that
/// member is `undefined`. Its value is the join of its writes, so every write
/// is a source of the name it stands for.
pub fn is_default_temp(var: &str) -> bool {
    var.starts_with("__dflt_")
}

/// Bound on temps read through temps (`get()()()`): lowering never makes a
/// cycle, the bound only keeps a malformed IR from looping.
const MAX_TEMP_DEPTH: usize = 8;

impl SourceNames {
    /// The temps bound anywhere in render `cfg` or the bodies of its `hooks`,
    /// their function literals included. `provenance` names a hook marker's
    /// callee. A call inlined into `cfg` left no call behind, only the
    /// variable the splice bound its return to: `inlined` lists those, as
    /// (variable, callee). `files` holds the text of every other temp.
    pub fn of<'a>(
        cfg: &CFG,
        hooks: &[HookEntry],
        provenance: &[HookProvenance],
        inlined: impl IntoIterator<Item = (&'a Var, &'a str)>,
        files: &FileTable,
    ) -> Self {
        let mut texts = HashMap::new();
        let mut callees = HashMap::new();
        for (var, callee) in inlined {
            if is_call_temp(var) {
                callees.insert(var.clone(), Expr::Var(callee.to_string()));
            }
        }
        let mut slots: HashMap<HookLabel, Var> = HashMap::new();
        let mut ranked: HashMap<HookLabel, (bool, bool, Var)> = HashMap::new();
        for stmt in cfg.blocks.values().flat_map(|b| &b.stmts) {
            // A slot an inlined hook declares (`storedValue#3`) is named by
            // what the component calls it, the alias table's business.
            let Stmt::Let { var, rhs, .. } = stmt else {
                continue;
            };
            if var.starts_with("__") || var.contains(SPLICE_MARK) {
                continue;
            }
            // The slot the name holds, or the slots the array it holds keeps
            // at its positions: a state hook's pair kept whole reads its
            // value as `t[0]` (#283), and so does a tuple `[v, setV]` or a
            // custom hook's returned pair, destructured or not (#288).
            let mut named: Vec<(HookLabel, Var)> = Vec::new();
            let name = Expr::Var(var.clone());
            let held = match rhs.peel_ts() {
                Expr::StateVal(_) => Some(rhs.peel_ts()),
                _ => element_binding(&name, cfg),
            };
            match held {
                Some(Expr::StateVal(label)) => named.push((*label, var.clone())),
                Some(Expr::ArrayLit { elems, .. }) => {
                    for (i, e) in elems.iter().enumerate() {
                        if let Some(Expr::StateVal(label)) = element_binding(e, cfg) {
                            named.push((*label, format!("{var}{PATH_MARK}{i}")));
                        }
                    }
                }
                _ => {}
            }
            // The name a declaration binds wins (`const [n] = …`, `const t =
            // useState(0)`) over one renaming another name (`const c = n`,
            // #278, unless nothing else names the slot: `const t =
            // useCounter()`), a name over a member of a stored array, then
            // the smallest.
            let renames = matches!(rhs.peel_ts(), Expr::Var(_));
            let rank = |n: &Var| (renames, n.contains(PATH_MARK), n.clone());
            for (label, name) in named {
                let key = rank(&name);
                match ranked.get(&label) {
                    Some(best) if *best <= key => {}
                    _ => {
                        ranked.insert(label, key);
                        slots.insert(label, name);
                    }
                }
            }
        }
        // The render, the bodies hook extraction took out of it, and every
        // function nested in either.
        let roots = std::iter::once(cfg).chain(hooks.iter().filter_map(HookEntry::body_cfg));
        let bodies = roots.flat_map(|root| {
            std::iter::once(root).chain(nested_bodies(root).into_iter().map(|(_, b)| b))
        });
        for body in bodies {
            for block in body.blocks.values() {
                for stmt in &block.stmts {
                    let Stmt::Let { var, rhs, span } = stmt else {
                        continue;
                    };
                    let unsalted = var.split_once(SPLICE_MARK).map_or(var.as_str(), |(v, _)| v);
                    if let Some(text) = span.and_then(|at| files.temp_text(at.file, unsalted)) {
                        texts.insert(var.clone(), excerpt(text));
                        continue;
                    }
                    if is_argument_temp(var) {
                        texts.insert(var.clone(), excerpt(&rhs.describe()));
                        continue;
                    }
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
        SourceNames {
            callees,
            texts,
            slots,
        }
    }

    /// The name the `useState` destructuring declares for slot `label`, as
    /// the source wrote it (#278).
    pub fn declared_slot(&self, label: HookLabel) -> Option<Cow<'_, str>> {
        self.slots.get(&label).map(|v| source_name(v))
    }

    /// [`source_name`], with a temp at the root read as what it stands for:
    /// `__t12_30` → `a ?? b`, a call temp as the call it binds
    /// (`__call_207` → `get(…)`, `__hook_0.set` → `useThing(…).set`), and a
    /// temp whose callee is itself a temp (`get()()`) → `get(…)(…)`.
    pub fn name<'a>(&'a self, var: &'a str) -> Cow<'a, str> {
        self.name_at(var, 0)
    }

    /// An expression as a message prints it: [`Expr::describe`], with every
    /// variable on its spine read through this table.
    pub fn describe(&self, e: &Expr) -> String {
        self.expr_at(e, 0)
    }

    fn name_at<'a>(&'a self, var: &'a str, depth: usize) -> Cow<'a, str> {
        let (root, path) = match var.find(PATH_MARK) {
            Some(at) => var.split_at(at),
            None => (var, ""),
        };
        if let Some(text) = self.texts.get(root) {
            if path.is_empty() {
                return Cow::Borrowed(text);
            }
            let mut out = format!("({text})");
            push_path(&mut out, path);
            return Cow::Owned(out);
        }
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
            Expr::FieldAccess { obj, field } => format!("{}.{field}", self.operand(obj, depth)),
            Expr::IndexAccess { arr, .. } => format!("{}[…]", self.operand(arr, depth)),
            Expr::Call { fn_, .. } => format!("{}(…)", self.operand(fn_, depth)),
            Expr::New { fn_, .. } => format!("new {}(…)", self.operand(fn_, depth)),
            other => other.describe(),
        }
    }

    /// [`Self::expr_at`] for the object of a member or a callee: a temp's
    /// text in parentheses (`(a ?? b).x`, not `a ?? b.x`).
    fn operand(&self, e: &Expr, depth: usize) -> String {
        match e.peel_ts() {
            Expr::Var(v) if self.texts.contains_key(v) => format!("({})", self.texts[v]),
            _ => self.expr_at(e, depth),
        }
    }
}

/// Longest source text, in characters, a message quotes for a temp.
const MAX_TEXT: usize = 40;

/// `text` on one line, cut after [`MAX_TEXT`] characters: a temp can stand
/// for a ternary whose arms are whole functions.
fn excerpt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(MAX_TEXT) {
        Some((at, _)) => format!("{}…", flat[..at].trim_end()),
        None => flat,
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
            &[],
            [(&"__call_207".to_string(), "get")],
            &FileTable::default(),
        );
        assert_eq!(names.name("__call_207"), "get(…)");
        assert_eq!(names.name("__call_207.1"), "get(…)[1]");
        assert_eq!(names.name("t.1"), "t[1]");
    }

    /// A temp reads as the source text it stands for, found in the file its
    /// `Let` sits in, whatever salt a splice gave it; a member of it keeps
    /// the text whole.
    #[test]
    fn a_value_temp_reads_as_its_source_text() {
        let mut files = FileTable::default();
        let file = files.intern(std::path::Path::new("a.tsx"));
        files.name_temp(file, "__t4_10".to_string(), "a  ??\n  b");
        let at = crate::ir::SourceRange {
            file,
            line: 1,
            col: 4,
        };
        let bind = |var: &str| Stmt::Let {
            var: var.to_string(),
            rhs: Expr::Var("a".to_string()),
            span: Some(at),
        };
        let cfg = crate::test_support::single_block_cfg(vec![bind("__t4_10"), bind("__t4_10#2")]);
        let names = SourceNames::of(&cfg, &[], &[], [], &files);
        assert_eq!(names.name("__t4_10"), "a ?? b");
        assert_eq!(names.name("__t4_10#2"), "a ?? b");
        assert_eq!(names.name("__t4_10.x"), "(a ?? b).x");
        assert_eq!(
            names.describe(&Expr::Call {
                id: crate::ir::expr::SYNTHETIC_CALL,
                fn_: Box::new(Expr::Var("__t4_10".to_string())),
                args: vec![],
            }),
            "(a ?? b)(…)"
        );
    }

    #[test]
    fn a_long_text_is_cut() {
        let long = "cond ? () => setFieldKeys((keys) => ({ ...keys, [path]: 1 })) : null";
        let short = excerpt(long);
        assert!(short.ends_with('…'));
        assert!(short.chars().count() <= MAX_TEXT + 1);
    }

    #[test]
    fn a_temp_whose_callee_is_a_temp_reads_through_it() {
        // `get()()(x)`: the inner call is inlined (no call left), the outer
        // one is a `Let` of a call whose callee is the inner temp.
        let outer = Stmt::Let {
            var: "__call_9".to_string(),
            rhs: Expr::Call {
                id: crate::ir::expr::SYNTHETIC_CALL,
                fn_: Box::new(Expr::Var("__call_5".to_string())),
                args: vec![],
            },
            span: None,
        };
        let names = SourceNames::of(
            &crate::test_support::single_block_cfg(vec![outer]),
            &[],
            &[],
            [(&"__call_5".to_string(), "get")],
            &FileTable::default(),
        );
        assert_eq!(names.name("__call_9"), "get(…)(…)");
    }
}
