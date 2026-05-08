use oxc_ast::ast::*;
use crate::core::abs_env::{AbsEnv, lookup};
use crate::core::aval::{AVal, CstValue, join};
use crate::events::{SetterArgClassif, ValueResolution};

pub fn eval_expr(env: &AbsEnv, expr: &Expression) -> AVal {
    match expr {
        Expression::BooleanLiteral(lit) => AVal::Cst(CstValue::Bool(lit.value)),
        Expression::NumericLiteral(lit) => AVal::Cst(CstValue::Num(lit.value)),
        Expression::StringLiteral(lit) => AVal::Cst(CstValue::Str(lit.value.as_str().to_owned())),
        Expression::NullLiteral(_) => AVal::Cst(CstValue::Null),
        Expression::TemplateLiteral(_) => AVal::String_,
        Expression::Identifier(id) => {
            if id.name == "undefined" {
                AVal::Cst(CstValue::Undefined)
            } else {
                lookup(env, id.name.as_str())
            }
        }
        Expression::BinaryExpression(bin) => eval_bop(env, bin),
        Expression::LogicalExpression(log) => eval_logical(env, log),
        Expression::UnaryExpression(un) => eval_unary(env, un),
        Expression::ConditionalExpression(cond) => {
            let t = eval_expr(env, &cond.consequent);
            let f = eval_expr(env, &cond.alternate);
            join(&t, &f)
        }
        Expression::AssignmentExpression(assign) => eval_expr(env, &assign.right),
        Expression::ArrowFunctionExpression(arrow) => {
            AVal::Clos(format!("clos_{}", arrow.span.start))
        }
        Expression::FunctionExpression(func) => {
            AVal::Clos(format!("clos_{}", func.span.start))
        }
        Expression::SequenceExpression(seq) => {
            seq.expressions.last().map_or(AVal::Top, |e| eval_expr(env, e))
        }
        Expression::TSTypeAssertion(a) => eval_expr(env, &a.expression),
        Expression::TSAsExpression(a) => eval_expr(env, &a.expression),
        Expression::TSSatisfiesExpression(a) => eval_expr(env, &a.expression),
        Expression::TSNonNullExpression(a) => eval_expr(env, &a.expression),
        _ => AVal::Top,
    }
}

fn eval_unary(env: &AbsEnv, un: &UnaryExpression) -> AVal {
    let val = eval_expr(env, &un.argument);
    match un.operator {
        UnaryOperator::LogicalNot => match val {
            AVal::Cst(CstValue::Bool(b)) => AVal::Cst(CstValue::Bool(!b)),
            AVal::Bool => AVal::Bool,
            _ => AVal::Top,
        },
        UnaryOperator::UnaryNegation => match val {
            AVal::Cst(CstValue::Num(n)) => AVal::Cst(CstValue::Num(-n)),
            AVal::Number => AVal::Number,
            _ => AVal::Top,
        },
        UnaryOperator::Typeof => AVal::String_,
        _ => AVal::Top,
    }
}

fn eval_bop(env: &AbsEnv, bin: &BinaryExpression) -> AVal {
    let l = eval_expr(env, &bin.left);
    let r = eval_expr(env, &bin.right);

    if let (AVal::Cst(cl), AVal::Cst(cr)) = (&l, &r) {
        if let Some(result) = fold_bop(bin.operator, cl, cr) {
            return result;
        }
    }

    match bin.operator {
        BinaryOperator::Addition => match (&l, &r) {
            (AVal::Number, AVal::Number)
            | (AVal::Cst(CstValue::Num(_)), AVal::Number)
            | (AVal::Number, AVal::Cst(CstValue::Num(_))) => AVal::Number,
            (AVal::String_, _) | (_, AVal::String_)
            | (AVal::Cst(CstValue::Str(_)), _) | (_, AVal::Cst(CstValue::Str(_))) => AVal::String_,
            _ => AVal::Top,
        },
        BinaryOperator::Subtraction
        | BinaryOperator::Multiplication
        | BinaryOperator::Division
        | BinaryOperator::Remainder => AVal::Number,
        BinaryOperator::StrictEquality
        | BinaryOperator::StrictInequality
        | BinaryOperator::Equality
        | BinaryOperator::Inequality
        | BinaryOperator::LessThan
        | BinaryOperator::LessEqualThan
        | BinaryOperator::GreaterThan
        | BinaryOperator::GreaterEqualThan => AVal::Bool,
        _ => AVal::Top,
    }
}

fn fold_bop(op: BinaryOperator, l: &CstValue, r: &CstValue) -> Option<AVal> {
    use CstValue::*;
    Some(match op {
        BinaryOperator::StrictEquality => AVal::Cst(Bool(l == r)),
        BinaryOperator::StrictInequality => AVal::Cst(Bool(l != r)),
        BinaryOperator::Addition => match (l, r) {
            (Num(a), Num(b)) => AVal::Cst(Num(a + b)),
            (Str(a), Str(b)) => AVal::Cst(Str(format!("{a}{b}"))),
            _ => return None,
        },
        BinaryOperator::Subtraction => match (l, r) {
            (Num(a), Num(b)) => AVal::Cst(Num(a - b)),
            _ => return None,
        },
        BinaryOperator::Multiplication => match (l, r) {
            (Num(a), Num(b)) => AVal::Cst(Num(a * b)),
            _ => return None,
        },
        BinaryOperator::Division => match (l, r) {
            (Num(a), Num(b)) => AVal::Cst(Num(a / b)),
            _ => return None,
        },
        BinaryOperator::LessThan => match (l, r) {
            (Num(a), Num(b)) => AVal::Cst(Bool(a < b)),
            _ => return None,
        },
        BinaryOperator::GreaterThan => match (l, r) {
            (Num(a), Num(b)) => AVal::Cst(Bool(a > b)),
            _ => return None,
        },
        _ => return None,
    })
}

fn eval_logical(env: &AbsEnv, log: &LogicalExpression) -> AVal {
    let l = eval_expr(env, &log.left);
    let r = eval_expr(env, &log.right);
    match log.operator {
        LogicalOperator::And => match &l {
            AVal::Cst(CstValue::Bool(false)) => l,
            AVal::Cst(CstValue::Bool(true)) => r,
            _ => join(&l, &r),
        },
        LogicalOperator::Or => match &l {
            AVal::Cst(CstValue::Bool(true)) => l,
            AVal::Cst(CstValue::Bool(false)) => r,
            _ => join(&l, &r),
        },
        LogicalOperator::Coalesce => match &l {
            AVal::Cst(CstValue::Null) | AVal::Cst(CstValue::Undefined) => r,
            AVal::Bot => r,
            _ => join(&l, &r),
        },
    }
}

pub fn resolve_value(env: &AbsEnv, expr: &Expression) -> ValueResolution {
    match eval_expr(env, expr) {
        AVal::Cst(v) => ValueResolution::Literal(v),
        _ => ValueResolution::Top,
    }
}

pub fn classify_setter_arg(env: &AbsEnv, arg: &Expression) -> (SetterArgClassif, ValueResolution) {
    let value = resolve_value(env, arg);

    let classif = match arg {
        Expression::ArrowFunctionExpression(arrow) => {
            classify_fn_body(&arrow.params, &arrow.body, arrow.expression)
        }
        Expression::FunctionExpression(func) => {
            match &func.body {
                Some(body) => classify_fn_body(&func.params, body, false),
                None => SetterArgClassif::Unknown,
            }
        }
        _ => match &value {
            ValueResolution::Literal(_) => SetterArgClassif::Constant,
            _ => SetterArgClassif::Unknown,
        },
    };

    (classif, value)
}

fn classify_fn_body(
    params: &FormalParameters,
    body: &FunctionBody,
    is_expression_body: bool,
) -> SetterArgClassif {
    let Some(first_param) = params.items.first() else {
        return SetterArgClassif::Constant;
    };
    let param_name = match &first_param.pattern {
        BindingPattern::BindingIdentifier(id) => id.name.as_str(),
        _ => return SetterArgClassif::Unknown,
    };

    let return_expr = extract_return_expr(body, is_expression_body);
    let Some(ret) = return_expr else {
        return SetterArgClassif::Unknown;
    };

    if is_just_identifier(ret, param_name) {
        SetterArgClassif::Identity
    } else if identifier_appears_in(param_name, ret) {
        SetterArgClassif::Functional
    } else {
        SetterArgClassif::Constant
    }
}

fn extract_return_expr<'a>(body: &'a FunctionBody<'a>, is_expression_body: bool) -> Option<&'a Expression<'a>> {
    if is_expression_body {
        // Concise arrow: single ExpressionStatement wrapping the return expr
        if let Some(Statement::ExpressionStatement(es)) = body.statements.first() {
            return Some(&es.expression);
        }
    }
    if body.statements.len() == 1 {
        if let Some(Statement::ReturnStatement(ret)) = body.statements.first() {
            return ret.argument.as_ref();
        }
    }
    None
}

fn is_just_identifier(expr: &Expression, name: &str) -> bool {
    matches!(expr, Expression::Identifier(id) if id.name == name)
}

pub fn identifier_appears_in(name: &str, expr: &Expression) -> bool {
    match expr {
        Expression::Identifier(id) => id.name == name,
        Expression::BinaryExpression(b) => {
            identifier_appears_in(name, &b.left) || identifier_appears_in(name, &b.right)
        }
        Expression::LogicalExpression(l) => {
            identifier_appears_in(name, &l.left) || identifier_appears_in(name, &l.right)
        }
        Expression::UnaryExpression(u) => identifier_appears_in(name, &u.argument),
        Expression::ConditionalExpression(c) => {
            identifier_appears_in(name, &c.test)
                || identifier_appears_in(name, &c.consequent)
                || identifier_appears_in(name, &c.alternate)
        }
        Expression::CallExpression(c) => c.arguments.iter().any(|a| {
            a.as_expression().map_or(false, |e| identifier_appears_in(name, e))
        }),
        Expression::ArrayExpression(a) => a.elements.iter().any(|el| {
            el.as_expression().map_or(false, |e| identifier_appears_in(name, e))
        }),
        _ => false,
    }
}
