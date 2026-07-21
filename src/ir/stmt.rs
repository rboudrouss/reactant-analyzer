use crate::ir::{
    expr::Expr,
    source_range::SourceRange,
    types::{Symbol, Var},
};

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        var: Var,
        rhs: Expr,
        span: Option<SourceRange>,
    },
    Assign {
        var: Var,
        rhs: Expr,
        span: Option<SourceRange>,
    },
    /// In-place write through a member expression: `obj.f = v`, `arr[i] = v`,
    /// `obj.f++`, `delete obj.f`. The heap identity of `obj` is unchanged —
    /// that is the semantic payload: a mutation, not a rebinding.
    MemberWrite {
        obj: Expr,
        key: MemberKey,
        rhs: Expr,
        span: Option<SourceRange>,
    },
    ExprStmt(Expr, Option<SourceRange>),
}

/// Which member of the object a [`Stmt::MemberWrite`] targets. `Index` keeps
/// the index expression alive: it is evaluated at runtime, so its reads count
/// (free variables, callbacks).
#[derive(Debug, Clone)]
pub enum MemberKey {
    Field(Symbol),
    Index(Expr),
}
