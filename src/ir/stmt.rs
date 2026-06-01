use crate::ir::{expr::Expr, types::Var};

#[derive(Debug)]
pub enum Stmt {
    Let { var: Var, rhs: Expr },
    Assign { var: Var, rhs: Expr },
    ExprStmt(Expr),
}
