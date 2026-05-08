use crate::core::aval::{AVal, join, leq, widen};
use std::collections::HashMap;

pub type AbsEnv = HashMap<String, AVal>;

pub fn lookup(env: &AbsEnv, name: &str) -> AVal {
    env.get(name).cloned().unwrap_or(AVal::Top)
}

pub fn extend(env: &AbsEnv, name: &str, val: AVal) -> AbsEnv {
    let mut out = env.clone();
    out.insert(name.to_owned(), val);
    out
}

pub fn join_env(a: &AbsEnv, b: &AbsEnv) -> AbsEnv {
    let mut out = HashMap::new();
    let all_keys = a.keys().chain(b.keys());
    for k in all_keys {
        if out.contains_key(k) {
            continue;
        }
        let va = a.get(k).unwrap_or(&AVal::Bot);
        let vb = b.get(k).unwrap_or(&AVal::Bot);
        out.insert(k.clone(), join(va, vb));
    }
    out
}

pub fn leq_env(a: &AbsEnv, b: &AbsEnv) -> bool {
    for (k, va) in a {
        let vb = b.get(k).unwrap_or(&AVal::Bot);
        if !leq(va, vb) {
            return false;
        }
    }
    true
}

pub fn widen_env(old: &AbsEnv, next: &AbsEnv) -> AbsEnv {
    let mut out = old.clone();
    for (k, vn) in next {
        let vo = old.get(k).unwrap_or(&AVal::Bot);
        out.insert(k.clone(), widen(vo, vn));
    }
    out
}

pub fn empty_env() -> AbsEnv {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::aval::CstValue;

    #[test]
    fn lookup_missing_is_top() {
        let env = empty_env();
        assert_eq!(lookup(&env, "x"), AVal::Top);
    }

    #[test]
    fn extend_adds_binding() {
        let env = empty_env();
        let env2 = extend(&env, "x", AVal::Number);
        assert_eq!(lookup(&env2, "x"), AVal::Number);
        assert_eq!(lookup(&env, "x"), AVal::Top); // original unchanged
    }

    #[test]
    fn join_env_pointwise() {
        let a = [
            ("x".to_string(), AVal::Cst(CstValue::Num(1.0))),
            ("y".to_string(), AVal::Number),
        ]
        .into();
        let b = [("x".to_string(), AVal::Cst(CstValue::Num(2.0)))].into();
        let j = join_env(&a, &b);
        assert_eq!(j["x"], AVal::Number);
        assert_eq!(j["y"], AVal::Number); // b missing y → Bot; join(Number, Bot) = Number
    }

    #[test]
    fn leq_env_holds() {
        let a: AbsEnv = [("x".to_string(), AVal::Cst(CstValue::Num(1.0)))].into();
        let b: AbsEnv = [("x".to_string(), AVal::Number)].into();
        assert!(leq_env(&a, &b));
        assert!(!leq_env(&b, &a));
    }

    #[test]
    fn widen_env_lifts_consts() {
        let old: AbsEnv = [("x".to_string(), AVal::Cst(CstValue::Num(1.0)))].into();
        let next: AbsEnv = [("x".to_string(), AVal::Cst(CstValue::Num(2.0)))].into();
        let w = widen_env(&old, &next);
        assert_eq!(w["x"], AVal::Number);
    }
}
