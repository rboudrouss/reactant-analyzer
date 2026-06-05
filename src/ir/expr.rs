use std::collections::HashMap;
use std::sync::Arc;

use crate::ir::{
    cfg::CFG,
    source_range::SourceRange,
    types::{ExprId, HookLabel, Symbol, Var},
};

#[derive(Debug, Clone)]
pub enum Prim {
    String(String),
    Int(i32),
    Float(f64),
    Bool(bool),
    Null,
    Unit,
}

#[derive(Debug, Clone)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Eq,
    Neq,
    Lt,
    Gt,
    Leq,
    Geq,
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TSType {
    Number,
    Boolean,
    Str,
    Reference,
    Unknown,
}

#[derive(Debug, Clone)]
pub enum Expr {
    // Primitive literals
    Lit(Prim),

    // Composites — each allocating node carries an ExprId (allocation-site key for the heap).
    ObjectLit {
        id: ExprId,
        fields: Vec<(Symbol, Expr)>,
    },
    ArrayLit {
        id: ExprId,
        elems: Vec<Expr>,
    },
    FnLit {
        id: ExprId,
        params: Vec<Var>,
        body_cfg: Arc<CFG>,
    },

    // Vars
    Var(Symbol),

    // Accesses
    FieldAccess {
        obj: Box<Expr>,
        field: Symbol,
    },
    IndexAccess {
        arr: Box<Expr>,
        idx: Box<Expr>,
    },

    // Ops
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        arg: Box<Expr>,
    },

    // Calls
    Call {
        fn_: Box<Expr>,
        args: Vec<Expr>,
    },
    CompApp {
        name: Symbol,
        props: Box<Expr>,
    },
    NativeElem {
        tag: Symbol,
        props: Box<Expr>,
        children: Vec<Expr>,
        /// Spans of JSX event-handler props (`onX={fn}`), keyed by prop name.
        /// Populated during lowering; consumed by `hook_extractor` to set `HookEntry::Handler.span`.
        prop_spans: HashMap<String, Option<SourceRange>>,
    },

    // TypeScript Annotations
    TSAnnotated(Box<Expr>, TSType),

    // React Hooks
    StateVal(HookLabel),
    StateSetter(HookLabel),
    MemoVal(HookLabel),
    CallbackVal(HookLabel),

    /// Injected by `expand_custom_hooks` for library hooks with a `HookSummary`.
    /// Evaluates directly to the encoded abstract value without going through the
    /// concrete expression language (avoids a circular dep between `ir` and `domains`).
    SummaryVal(SummaryValue),
}

/// Coarse abstract return-value hint for library hooks.
/// Lives in `ir` to avoid a circular dependency between `ir` and `domains`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryValue {
    /// ⊤ — completely unknown; default for hooks without a precise summary.
    Top,
    /// Hook returns a reference-stable value (safe as a `useEffect` dep).
    StableRef,
    /// Hook returns a reference-unstable value (new object/array every render).
    UnstableRef,
}

impl Expr {
    /// Returns `true` iff the expression tree contains no `Call` or `CompApp` node.
    /// Used to guard the `derived-state` rule: if the setter arg is call-free, the
    /// derivation is a pure data transformation and replacing the effect with `useMemo`
    /// is always safe.
    pub fn is_call_free(&self) -> bool {
        match self {
            Expr::Call { .. } | Expr::CompApp { .. } | Expr::NativeElem { .. } => false,
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::StateVal(_)
            | Expr::StateSetter(_)
            | Expr::MemoVal(_)
            | Expr::CallbackVal(_) => true,
            Expr::ObjectLit { fields, .. } => fields.iter().all(|(_, v)| v.is_call_free()),
            Expr::ArrayLit { elems, .. } => elems.iter().all(|e| e.is_call_free()),
            Expr::FnLit { .. } => true,
            Expr::FieldAccess { obj, .. } => obj.is_call_free(),
            Expr::IndexAccess { arr, idx } => arr.is_call_free() && idx.is_call_free(),
            Expr::BinOp { lhs, rhs, .. } => lhs.is_call_free() && rhs.is_call_free(),
            Expr::UnaryOp { arg, .. } => arg.is_call_free(),
            Expr::TSAnnotated(inner, _) => inner.is_call_free(),
            Expr::SummaryVal(_) => true,
        }
    }
}
