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

impl Stmt {
    /// Where the statement is in the source, whichever variant it is.
    ///
    /// `None` for a statement lowering or the splice synthesised and could not
    /// place — every such site is meant to place what it mints (ADR-039), so a
    /// `None` here is a bug to fix, not a shape to route around.
    pub fn span(&self) -> Option<SourceRange> {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::MemberWrite { span, .. }
            | Stmt::ExprStmt(_, span) => *span,
        }
    }
}

/// Which member of the object a [`Stmt::MemberWrite`] targets. `Index` keeps
/// the index expression alive: it is evaluated at runtime, so its reads count
/// (free variables, callbacks).
#[derive(Debug, Clone)]
pub enum MemberKey {
    Field(Symbol),
    Index(Expr),
}
