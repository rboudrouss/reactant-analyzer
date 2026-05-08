use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub enum CstValue {
    Num(f64),
    Bool(bool),
    Str(String),
    Null,
    Undefined,
}

impl PartialEq for CstValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CstValue::Num(a), CstValue::Num(b)) => a.to_bits() == b.to_bits(),
            (CstValue::Bool(a), CstValue::Bool(b)) => a == b,
            (CstValue::Str(a), CstValue::Str(b)) => a == b,
            (CstValue::Null, CstValue::Null) => true,
            (CstValue::Undefined, CstValue::Undefined) => true,
            _ => false,
        }
    }
}

impl Eq for CstValue {}

impl Hash for CstValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            CstValue::Num(v) => {
                0u8.hash(state);
                v.to_bits().hash(state);
            }
            CstValue::Bool(v) => {
                1u8.hash(state);
                v.hash(state);
            }
            CstValue::Str(v) => {
                2u8.hash(state);
                v.hash(state);
            }
            CstValue::Null => 3u8.hash(state),
            CstValue::Undefined => 4u8.hash(state),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AVal {
    Bot,
    Cst(CstValue),
    Number,
    Bool,
    String_,
    Clos(String),
    Setter(String),
    Top,
}

fn lift_cst(v: &CstValue) -> AVal {
    match v {
        CstValue::Num(_) => AVal::Number,
        CstValue::Bool(_) => AVal::Bool,
        CstValue::Str(_) => AVal::String_,
        CstValue::Null | CstValue::Undefined => AVal::Top,
    }
}

pub fn join(a: &AVal, b: &AVal) -> AVal {
    match (a, b) {
        (AVal::Bot, x) | (x, AVal::Bot) => x.clone(),
        (AVal::Top, _) | (_, AVal::Top) => AVal::Top,
        (AVal::Cst(v1), AVal::Cst(v2)) => {
            if v1 == v2 {
                AVal::Cst(v1.clone())
            } else {
                let lifted = lift_cst(v1);
                let lifted2 = lift_cst(v2);
                if lifted == lifted2 { lifted } else { AVal::Top }
            }
        }
        (AVal::Cst(v), t) | (t, AVal::Cst(v)) => {
            if &lift_cst(v) == t {
                t.clone()
            } else {
                AVal::Top
            }
        }
        (AVal::Number, AVal::Number) => AVal::Number,
        (AVal::Bool, AVal::Bool) => AVal::Bool,
        (AVal::String_, AVal::String_) => AVal::String_,
        (AVal::Clos(l1), AVal::Clos(l2)) => {
            if l1 == l2 {
                AVal::Clos(l1.clone())
            } else {
                AVal::Top
            }
        }
        (AVal::Setter(l1), AVal::Setter(l2)) => {
            if l1 == l2 {
                AVal::Setter(l1.clone())
            } else {
                AVal::Top
            }
        }
        _ => AVal::Top,
    }
}

pub fn leq(a: &AVal, b: &AVal) -> bool {
    match (a, b) {
        (AVal::Bot, _) => true,
        (_, AVal::Top) => true,
        (AVal::Cst(v1), AVal::Cst(v2)) => v1 == v2,
        (AVal::Cst(v), t) => &lift_cst(v) == t,
        (AVal::Number, AVal::Number) => true,
        (AVal::Bool, AVal::Bool) => true,
        (AVal::String_, AVal::String_) => true,
        (AVal::Clos(l1), AVal::Clos(l2)) => l1 == l2,
        (AVal::Setter(l1), AVal::Setter(l2)) => l1 == l2,
        _ => false,
    }
}

pub fn widen(old: &AVal, next: &AVal) -> AVal {
    match (old, next) {
        (AVal::Cst(v1), AVal::Cst(v2)) if v1 != v2 => lift_cst(v1),
        _ => join(old, next),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_same_const() {
        assert_eq!(
            join(
                &AVal::Cst(CstValue::Num(1.0)),
                &AVal::Cst(CstValue::Num(1.0))
            ),
            AVal::Cst(CstValue::Num(1.0))
        );
    }

    #[test]
    fn join_different_num_consts() {
        assert_eq!(
            join(
                &AVal::Cst(CstValue::Num(1.0)),
                &AVal::Cst(CstValue::Num(2.0))
            ),
            AVal::Number
        );
    }

    #[test]
    fn join_cross_type() {
        assert_eq!(
            join(
                &AVal::Cst(CstValue::Num(1.0)),
                &AVal::Cst(CstValue::Str("x".into()))
            ),
            AVal::Top
        );
    }

    #[test]
    fn join_bot_identity() {
        assert_eq!(join(&AVal::Bot, &AVal::Number), AVal::Number);
        assert_eq!(
            join(&AVal::Cst(CstValue::Bool(true)), &AVal::Bot),
            AVal::Cst(CstValue::Bool(true))
        );
    }

    #[test]
    fn join_top_absorbing() {
        assert_eq!(join(&AVal::Number, &AVal::Top), AVal::Top);
        assert_eq!(join(&AVal::Top, &AVal::Cst(CstValue::Num(1.0))), AVal::Top);
    }

    #[test]
    fn widen_different_consts_lifts() {
        assert_eq!(
            widen(
                &AVal::Cst(CstValue::Num(1.0)),
                &AVal::Cst(CstValue::Num(2.0))
            ),
            AVal::Number
        );
    }

    #[test]
    fn widen_same_const_keeps() {
        assert_eq!(
            widen(
                &AVal::Cst(CstValue::Num(1.0)),
                &AVal::Cst(CstValue::Num(1.0))
            ),
            AVal::Cst(CstValue::Num(1.0))
        );
    }

    #[test]
    fn leq_bot_least() {
        assert!(leq(&AVal::Bot, &AVal::Number));
        assert!(leq(&AVal::Bot, &AVal::Top));
        assert!(leq(&AVal::Bot, &AVal::Bot));
    }

    #[test]
    fn leq_top_greatest() {
        assert!(leq(&AVal::Number, &AVal::Top));
        assert!(leq(&AVal::Cst(CstValue::Num(1.0)), &AVal::Top));
    }

    #[test]
    fn leq_cst_to_type() {
        assert!(leq(&AVal::Cst(CstValue::Num(1.0)), &AVal::Number));
        assert!(!leq(&AVal::Cst(CstValue::Num(1.0)), &AVal::Bool));
        assert!(!leq(&AVal::Number, &AVal::Cst(CstValue::Num(1.0))));
    }

    #[test]
    fn join_cst_and_type() {
        assert_eq!(
            join(&AVal::Cst(CstValue::Num(1.0)), &AVal::Number),
            AVal::Number
        );
        assert_eq!(
            join(&AVal::Cst(CstValue::Bool(true)), &AVal::Number),
            AVal::Top
        );
    }

    #[test]
    fn join_closures() {
        assert_eq!(
            join(&AVal::Clos("a".into()), &AVal::Clos("a".into())),
            AVal::Clos("a".into())
        );
        assert_eq!(
            join(&AVal::Clos("a".into()), &AVal::Clos("b".into())),
            AVal::Top
        );
    }
}
