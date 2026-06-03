use crate::ir::{expr::Expr, source_range::SourceRange, types::Var};

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
    ExprStmt(Expr, Option<SourceRange>),
}
