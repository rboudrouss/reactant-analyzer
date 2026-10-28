//! Which calls compute the same content from the same inputs (#224).
//!
//! A call's result holds while its inputs hold only when its callee is a
//! function of them. The engine knows three kinds of callee. A builtin of the
//! language this table names is one: `JSON.stringify`, `String`, `Math.max`, a
//! method of a receiver whose kind is known — a string's `trim`, an array's
//! `join`, a `Date`'s `getTime`, a `URLSearchParams`' `get` — pure of its
//! receiver and its arguments. A utility the analysis resolves is spliced at
//! its call (`fixpoint::splice_one_call`), so its body's own calls are what is
//! read, and the call never reaches here. Anything else may differ: a builtin
//! the table leaves out on purpose (`Math.random`, `Date.now`, `new Date()`,
//! `performance.now`, `crypto.randomUUID`, `Symbol()`), an import, a closure, a
//! method by name alone on a receiver of unknown kind.
//!
//! Content, not identity: whether the result is a new reference is the value
//! domain's question, so a builtin that may return a reference is listed only
//! when the domain knows that reference fresh (`returns_fresh_reference`,
//! `new`) — `find`, `at`, `reduce` and `Map.get` are not.
//!
//! A kind is a syntactic fact the caller computes ([`Kind`]): a literal, a
//! known constructor, a known call's result, a library hook's documented
//! result (a router's `useSearchParams`), or a free name of the language's
//! global scope. A builtin that runs a function argument (`map`, `filter`,
//! `replace`) is pure only when that argument is a closure whose own calls
//! are ([`body_holds`]) or a value of a known kind. The conversions a builtin
//! makes implicitly (`toString`, `toJSON` on an object argument) are taken as
//! deterministic.

use std::collections::HashSet;

use crate::ir::{
    cfg::CFG,
    expr::{BinOp, Expr, Prim, UnaryOp},
    free_vars::compute_free_vars,
    types::Var,
};

/// What a value is known to be, for the builtins [`call`] can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Unknown,
    /// A free name of the global scope the table knows (`JSON`, `Math`,
    /// `String`, `URLSearchParams`, …), bound nowhere in the program's view.
    Global(&'static str),
    Str,
    Num,
    Bool,
    Array,
    Date,
    /// A `URLSearchParams`.
    Params,
    /// A pair whose first element is a `URLSearchParams` (react-router's
    /// `useSearchParams()`).
    ParamsPair,
    /// A `Map` or a `Set`.
    Keyed,
}

impl Kind {
    /// The kind on both paths of a join.
    pub fn join(self, other: Kind) -> Kind {
        if self == other { self } else { Kind::Unknown }
    }

    /// A value, not a function a builtin could call.
    fn is_data(self) -> bool {
        !matches!(self, Kind::Unknown | Kind::Global(_))
    }
}

/// The global names the table knows. A free name reads as one only when
/// nothing in scope binds it — the caller's question.
const GLOBALS: &[&str] = &[
    "JSON",
    "Math",
    "Object",
    "Array",
    "Number",
    "String",
    "Boolean",
    "Date",
    "URLSearchParams",
    "Map",
    "Set",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "encodeURIComponent",
    "decodeURIComponent",
    "encodeURI",
    "decodeURI",
];

/// The kind of a free name nothing in scope binds.
pub fn global(name: &str) -> Kind {
    GLOBALS
        .iter()
        .find(|g| **g == name)
        .map_or(Kind::Unknown, |g| Kind::Global(g))
}

/// One pure builtin: called on `on`, named `name`, it returns a value of
/// kind `gives`, and runs its argument at `runs` when there is one.
struct Builtin {
    on: Kind,
    name: &'static str,
    gives: Kind,
    runs: Option<usize>,
}

const fn b(on: Kind, name: &'static str, gives: Kind) -> Builtin {
    Builtin {
        on,
        name,
        gives,
        runs: None,
    }
}

const fn cb(on: Kind, name: &'static str, gives: Kind, runs: usize) -> Builtin {
    Builtin {
        on,
        name,
        gives,
        runs: Some(runs),
    }
}

use Kind::{
    Array as A, Bool as B, Date as D, Keyed as K, Num as N, Params as P, Str as S, Unknown as U,
};

/// A bare call of a global (`String(x)`). `Date()` and `Symbol()` are absent:
/// the time, a fresh symbol.
const CALLS: &[Builtin] = &[
    b(Kind::Global("String"), "", S),
    b(Kind::Global("Number"), "", N),
    b(Kind::Global("Boolean"), "", B),
    b(Kind::Global("parseInt"), "", N),
    b(Kind::Global("parseFloat"), "", N),
    b(Kind::Global("isNaN"), "", B),
    b(Kind::Global("isFinite"), "", B),
    b(Kind::Global("encodeURIComponent"), "", S),
    b(Kind::Global("decodeURIComponent"), "", S),
    b(Kind::Global("encodeURI"), "", S),
    b(Kind::Global("decodeURI"), "", S),
];

/// A method call `recv.name(…)`. `Math.random` and `Date.now` are absent, so
/// are the mutating methods (`sort`, `push`, `set`): a call that moves its
/// receiver is no function of it.
const METHODS: &[Builtin] = &[
    b(Kind::Global("JSON"), "stringify", S),
    b(Kind::Global("JSON"), "parse", U),
    b(Kind::Global("Object"), "keys", A),
    b(Kind::Global("Object"), "values", A),
    b(Kind::Global("Object"), "entries", A),
    b(Kind::Global("Object"), "fromEntries", U),
    b(Kind::Global("Object"), "is", B),
    b(Kind::Global("Object"), "hasOwn", B),
    b(Kind::Global("Array"), "isArray", B),
    b(Kind::Global("Array"), "of", A),
    cb(Kind::Global("Array"), "from", A, 1),
    b(Kind::Global("Number"), "isInteger", B),
    b(Kind::Global("Number"), "isFinite", B),
    b(Kind::Global("Number"), "isNaN", B),
    b(Kind::Global("Number"), "isSafeInteger", B),
    b(Kind::Global("Number"), "parseInt", N),
    b(Kind::Global("Number"), "parseFloat", N),
    b(Kind::Global("String"), "fromCharCode", S),
    b(Kind::Global("Date"), "parse", N),
    b(Kind::Global("Date"), "UTC", N),
    b(Kind::Global("Math"), "abs", N),
    b(Kind::Global("Math"), "ceil", N),
    b(Kind::Global("Math"), "floor", N),
    b(Kind::Global("Math"), "round", N),
    b(Kind::Global("Math"), "trunc", N),
    b(Kind::Global("Math"), "sign", N),
    b(Kind::Global("Math"), "min", N),
    b(Kind::Global("Math"), "max", N),
    b(Kind::Global("Math"), "pow", N),
    b(Kind::Global("Math"), "sqrt", N),
    b(Kind::Global("Math"), "cbrt", N),
    b(Kind::Global("Math"), "exp", N),
    b(Kind::Global("Math"), "log", N),
    b(Kind::Global("Math"), "log2", N),
    b(Kind::Global("Math"), "log10", N),
    b(Kind::Global("Math"), "hypot", N),
    b(Kind::Global("Math"), "sin", N),
    b(Kind::Global("Math"), "cos", N),
    b(Kind::Global("Math"), "tan", N),
    b(Kind::Global("Math"), "atan2", N),
    // A string.
    b(S, "toString", S),
    b(S, "valueOf", S),
    b(S, "trim", S),
    b(S, "trimStart", S),
    b(S, "trimEnd", S),
    b(S, "toLowerCase", S),
    b(S, "toUpperCase", S),
    b(S, "slice", S),
    b(S, "substring", S),
    b(S, "substr", S),
    b(S, "padStart", S),
    b(S, "padEnd", S),
    b(S, "repeat", S),
    b(S, "concat", S),
    b(S, "charAt", S),
    b(S, "at", S),
    b(S, "normalize", S),
    b(S, "split", A),
    b(S, "includes", B),
    b(S, "startsWith", B),
    b(S, "endsWith", B),
    b(S, "indexOf", N),
    b(S, "lastIndexOf", N),
    b(S, "charCodeAt", N),
    b(S, "localeCompare", N),
    cb(S, "replace", S, 1),
    cb(S, "replaceAll", S, 1),
    // A number, a boolean.
    b(N, "toString", S),
    b(N, "toFixed", S),
    b(N, "toPrecision", S),
    b(N, "valueOf", N),
    b(B, "toString", S),
    // An array.
    b(A, "join", S),
    b(A, "includes", B),
    b(A, "indexOf", N),
    b(A, "lastIndexOf", N),
    b(A, "flat", A),
    b(A, "toReversed", A),
    cb(A, "map", A, 0),
    cb(A, "filter", A, 0),
    cb(A, "flatMap", A, 0),
    cb(A, "toSorted", A, 0),
    cb(A, "some", B, 0),
    cb(A, "every", B, 0),
    cb(A, "findIndex", N, 0),
    cb(A, "findLastIndex", N, 0),
    // A date.
    b(D, "getTime", N),
    b(D, "valueOf", N),
    b(D, "getFullYear", N),
    b(D, "getMonth", N),
    b(D, "getDate", N),
    b(D, "getDay", N),
    b(D, "getHours", N),
    b(D, "getMinutes", N),
    b(D, "getSeconds", N),
    b(D, "getMilliseconds", N),
    b(D, "getUTCFullYear", N),
    b(D, "getUTCMonth", N),
    b(D, "getUTCDate", N),
    b(D, "getUTCDay", N),
    b(D, "getUTCHours", N),
    b(D, "getTimezoneOffset", N),
    b(D, "toISOString", S),
    b(D, "toJSON", S),
    b(D, "toString", S),
    b(D, "toDateString", S),
    b(D, "toUTCString", S),
    b(D, "toLocaleDateString", S),
    b(D, "toLocaleTimeString", S),
    b(D, "toLocaleString", S),
    // A `URLSearchParams`, a `Map`, a `Set`.
    b(P, "get", S),
    b(P, "has", B),
    b(P, "toString", S),
    b(K, "has", B),
];

/// `new C(…)`. A fresh object each time — identity is the value's question
/// (`Stability::PerRender`) — whose contents are a function of the
/// arguments. `new Date()` with none is the time.
const NEWS: &[Builtin] = &[
    b(Kind::Global("Date"), "", D),
    b(Kind::Global("URLSearchParams"), "", P),
    b(Kind::Global("Map"), "", K),
    b(Kind::Global("Set"), "", K),
];

/// A library hook whose result's kind its package documents: `(package,
/// hook, kind)`. A router's search params are a `URLSearchParams`.
const HOOKS: &[(&str, &str, Kind)] = &[
    ("next/navigation", "useSearchParams", Kind::Params),
    ("react-router", "useSearchParams", Kind::ParamsPair),
    ("react-router-dom", "useSearchParams", Kind::ParamsPair),
];

/// The kind of the result of hook `name` imported from `package`.
pub fn hook_result(package: Option<&str>, name: &str) -> Kind {
    HOOKS
        .iter()
        .find(|(p, h, _)| package == Some(*p) && *h == name)
        .map_or(Kind::Unknown, |(_, _, k)| *k)
}

/// The kind of element `idx` of a value of kind `of`.
pub fn element(of: Kind, idx: &Expr) -> Kind {
    match (of, idx.peel_ts()) {
        (Kind::ParamsPair, Expr::Lit(Prim::Int(0))) => Kind::Params,
        _ => Kind::Unknown,
    }
}

/// What a call is made through.
#[derive(Debug, Clone, Copy)]
pub enum Callee<'e> {
    /// `f(…)`, with the kind of `f`.
    Value(Kind),
    /// `recv.name(…)`, with the kind of `recv`.
    Method(Kind, &'e str),
}

impl<'e> Callee<'e> {
    /// The callee of a call through `fn_`, given the kind of an expression.
    pub fn of(fn_: &'e Expr, kind: &mut dyn FnMut(&'e Expr) -> Kind) -> Self {
        match fn_.peel_ts() {
            Expr::FieldAccess { obj, field } => Callee::Method(kind(obj), field),
            other => Callee::Value(kind(other)),
        }
    }
}

/// The kind of a known builtin's result when the call returns the same value
/// on the same inputs, `None` when it may differ. `arg` is the kind of the
/// argument at an index; `var` the kind of a free name of a closure argument
/// the builtin runs, as the caller sees it.
pub fn call(
    callee: Callee<'_>,
    new: bool,
    args: &[Expr],
    arg: &dyn Fn(usize) -> Kind,
    var: &dyn Fn(&str) -> Kind,
) -> Option<Kind> {
    let found = match (callee, new) {
        (Callee::Value(k), false) => CALLS.iter().find(|x| x.on == k),
        (Callee::Value(k), true) => NEWS
            .iter()
            .find(|x| x.on == k)
            .filter(|_| k != Kind::Global("Date") || !args.is_empty()),
        (Callee::Method(k, name), false) => METHODS.iter().find(|x| x.on == k && x.name == name),
        (Callee::Method(..), true) => None,
    }?;
    let ran = found.runs.and_then(|i| args.get(i).map(|a| (i, a)));
    let runs_known = ran.is_none_or(|(i, a)| match a.peel_ts() {
        Expr::FnLit {
            params, body_cfg, ..
        } => body_holds(body_cfg, params, var),
        _ => arg(i).is_data(),
    });
    runs_known.then_some(found.gives)
}

/// Does every call `body` runs, closures it only builds aside, return the
/// same value on the same inputs? Its own names have no known kind; a free
/// one has the kind `var` gives it.
pub fn body_holds(body: &CFG, params: &[Var], var: &dyn Fn(&str) -> Kind) -> bool {
    let free: HashSet<Var> = compute_free_vars(body);
    let inner = |v: &str| {
        if params.iter().any(|p| p == v) || !free.contains(v) {
            Kind::Unknown
        } else {
            var(v)
        }
    };
    fn walk(e: &Expr, var: &dyn Fn(&str) -> Kind) -> bool {
        match e {
            // Built, not run.
            Expr::FnLit { .. } => true,
            Expr::Call { fn_, args } | Expr::New { fn_, args, .. } => {
                let callee = Callee::of(fn_, &mut |x| kind_of(x, var));
                call(
                    callee,
                    matches!(e, Expr::New { .. }),
                    args,
                    &|i| kind_of(&args[i], var),
                    var,
                )
                .is_some()
                    && walk(fn_, var)
                    && args.iter().all(|a| walk(a, var))
            }
            _ => {
                let mut ok = true;
                e.for_each_child(&mut |c| ok = ok && walk(c, var));
                ok
            }
        }
    }
    let mut ok = true;
    body.for_each_expr(&mut |e| ok = ok && walk(e, &inner));
    ok
}

/// The kind of `e` read syntactically, a name's kind given by `var`.
pub fn kind_of(e: &Expr, var: &dyn Fn(&str) -> Kind) -> Kind {
    match e {
        Expr::Lit(p) => lit(p),
        Expr::Var(v) => var(v),
        Expr::ArrayLit { .. } => Kind::Array,
        Expr::IndexAccess { arr, idx } => element(kind_of(arr, var), idx),
        Expr::TSAnnotated(inner) => kind_of(inner, var),
        Expr::BinOp { op, lhs, rhs } => binop(op, kind_of(lhs, var), kind_of(rhs, var)),
        Expr::UnaryOp { op, .. } => unary(op),
        Expr::Call { fn_, args } | Expr::New { fn_, args, .. } => {
            let callee = Callee::of(fn_, &mut |x| kind_of(x, var));
            call(
                callee,
                matches!(e, Expr::New { .. }),
                args,
                &|i| kind_of(&args[i], var),
                var,
            )
            .unwrap_or_default()
        }
        _ => Kind::Unknown,
    }
}

/// The kind of a literal.
pub fn lit(p: &Prim) -> Kind {
    match p {
        Prim::String(_) => Kind::Str,
        Prim::Int(_) | Prim::Float(_) => Kind::Num,
        Prim::Bool(_) => Kind::Bool,
        Prim::Null | Prim::Unit => Kind::Unknown,
    }
}

/// The kind of `l op r`.
pub fn binop(op: &BinOp, l: Kind, r: Kind) -> Kind {
    match op {
        BinOp::Add if l == Kind::Str || r == Kind::Str => Kind::Str,
        BinOp::Add if l == Kind::Num && r == Kind::Num => Kind::Num,
        BinOp::Add | BinOp::And | BinOp::Or => Kind::Unknown,
        BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr
        | BinOp::UShr => Kind::Num,
        BinOp::Eq
        | BinOp::Neq
        | BinOp::Lt
        | BinOp::Gt
        | BinOp::Leq
        | BinOp::Geq
        | BinOp::In
        | BinOp::InstanceOf => Kind::Bool,
    }
}

/// The kind of `op x`.
pub fn unary(op: &UnaryOp) -> Kind {
    match op {
        UnaryOp::Not => Kind::Bool,
        UnaryOp::TypeOf => Kind::Str,
        UnaryOp::Neg | UnaryOp::Plus | UnaryOp::BitNot => Kind::Num,
        UnaryOp::Unknown => Kind::Unknown,
    }
}
