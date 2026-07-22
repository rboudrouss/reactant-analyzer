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

    // Composites each allocating node carries an ExprId (allocation-site key for the heap).
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

    /// Marks the call site of a hook whose result carries no tracked value
    /// (`useEffect`, `useRef`, custom hooks, …). Evaluates to unit; its only
    /// role is to keep the hook's label anchored in the CFG so call-site
    /// blocks survive inlining and renumbering (`collect_hook_calls`,
    /// conditional-hook). Every extracted hook leaves its label in the CFG —
    /// value-bearing kinds via `StateVal`/`MemoVal`/…, all others via this.
    HookMarker(HookLabel),

    /// Injected by `expand_custom_hooks` for library hooks with a `HookSummary`.
    /// Evaluates directly to the encoded abstract value without going through the
    /// concrete expression language (avoids a circular dep between `ir` and `domains`).
    SummaryVal(SummaryValue),
}

/// Coarse abstract return-value hint for library hooks.
/// Lives in `ir` to avoid a circular dependency between `ir` and `domains`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryValue {
    /// ⊤ completely unknown; default for hooks without a precise summary.
    Top,
    /// Hook returns a reference-stable value (safe as a `useEffect` dep).
    StableRef,
    /// Hook returns a reference-unstable value (new object/array every render).
    UnstableRef,
}

impl Expr {
    /// Apply `f` to every DIRECT child expression of `self`.
    ///
    /// This is the canonical child enumeration: walkers write only the arms
    /// they treat specially and delegate the default case here, so a new
    /// `Expr` variant breaks compilation in exactly one place and every
    /// walker inherits the fix. The match is deliberately exhaustive — do
    /// NOT add a `_` arm.
    ///
    /// `FnLit` bodies are CFGs, not child expressions: crossing the function
    /// boundary is a per-walker decision (see [`crate::ir::cfg::CFG::for_each_expr`]).
    pub fn for_each_child<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::FnLit { .. }
            | Expr::StateVal(_)
            | Expr::StateSetter(_)
            | Expr::MemoVal(_)
            | Expr::CallbackVal(_)
            | Expr::HookMarker(_)
            | Expr::SummaryVal(_) => {}
            Expr::ObjectLit { fields, .. } => {
                for (_, v) in fields {
                    f(v);
                }
            }
            Expr::ArrayLit { elems, .. } => {
                for e in elems {
                    f(e);
                }
            }
            Expr::FieldAccess { obj, .. } => f(obj),
            Expr::IndexAccess { arr, idx } => {
                f(arr);
                f(idx);
            }
            Expr::BinOp { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            Expr::UnaryOp { arg, .. } => f(arg),
            Expr::Call { fn_, args } => {
                f(fn_);
                for a in args {
                    f(a);
                }
            }
            Expr::CompApp { props, .. } => f(props),
            Expr::NativeElem {
                props, children, ..
            } => {
                f(props);
                for c in children {
                    f(c);
                }
            }
            Expr::TSAnnotated(inner, _) => f(inner),
        }
    }

    /// Returns `true` iff the expression tree contains no `Call` or `CompApp` node.
    /// `FnLit` bodies are not crossed (they are leaves for `for_each_child`).
    pub fn is_call_free(&self) -> bool {
        match self {
            Expr::Call { .. } | Expr::CompApp { .. } | Expr::NativeElem { .. } => false,
            _ => {
                let mut free = true;
                self.for_each_child(&mut |c| free &= c.is_call_free());
                free
            }
        }
    }

    /// Strip any `TSAnnotated` wrappers, returning the underlying expression.
    pub fn peel_ts(&self) -> &Expr {
        let mut e = self;
        while let Expr::TSAnnotated(inner, _) = e {
            e = inner;
        }
        e
    }
}
