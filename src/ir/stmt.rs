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

    /// Apply `f` to every expression the statement evaluates: a binding's
    /// right-hand side; a member write's object, index and value; an
    /// expression statement's expression. The one place that knows a
    /// statement's operands, so a walker over a body's expressions cannot
    /// forget one (a member write's index was dropped by two of them).
    pub fn for_each_expr<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        match self {
            Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => f(rhs),
            Stmt::MemberWrite { obj, key, rhs, .. } => {
                f(obj);
                if let MemberKey::Index(idx) = key {
                    f(idx);
                }
                f(rhs);
            }
            Stmt::ExprStmt(e, _) => f(e),
        }
    }

    /// [`Self::for_each_expr`], mutably — the same operands.
    pub fn for_each_expr_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match self {
            Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => f(rhs),
            Stmt::MemberWrite { obj, key, rhs, .. } => {
                f(obj);
                if let MemberKey::Index(idx) = key {
                    f(idx);
                }
                f(rhs);
            }
            Stmt::ExprStmt(e, _) => f(e),
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
