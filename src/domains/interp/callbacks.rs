use crate::{
    domains::{AbstractDomain, stores::AbstractEnv},
    ir::expr::Expr,
};

/// How a call's closure arguments should be treated by the side-effect pre-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerClass {
    /// Callee is a bound state setter handled by the core exec path (functional
    /// updaters), so the pre-pass must NOT descend its closure.
    Setter,
    /// Runs as a consequence of the current render/effect: synchronous HOFs
    /// (`map`, `forEach`, …) and scheduled async (`.then`/`.catch`/`.finally`,
    /// `setTimeout`/`setInterval`, `queueMicrotask`, `requestAnimationFrame`).
    /// Its closure arguments ARE descended into.
    InCycle,
    /// Event subscription (`addEventListener`/`removeEventListener`) triggered
    /// externally, NOT part of the render→effect→render cycle. Not descended.
    Subscription,
    /// Unrecognized callee (custom helper/hook). Conservatively NOT descended
    /// (FP-averse: avoids flagging custom subscription wrappers).
    Unknown,
}

/// Classify a call's callee to decide whether its closure arguments run as a
/// consequence of the current render/effect (and so must be descended into for
/// their side effects).
pub fn classify_callee<D: AbstractDomain>(fn_: &Expr, env: &AbstractEnv<D>) -> TriggerClass {
    match fn_ {
        Expr::Var(name) => {
            if env.setter_label(name).is_some() {
                TriggerClass::Setter
            } else {
                match name.as_str() {
                    "setTimeout" | "setInterval" | "queueMicrotask" | "requestAnimationFrame" => {
                        TriggerClass::InCycle
                    }
                    _ => TriggerClass::Unknown,
                }
            }
        }
        Expr::FieldAccess { obj, field } => match field.as_str() {
            "then" | "catch" | "finally" | "allSettled" | "any" => TriggerClass::InCycle,
            "map" | "forEach" | "reduce" | "filter" | "find" | "flatMap" | "some" | "every"
            | "findIndex" | "findLast" | "findLastIndex" | "reduceRight" | "sort" | "toSorted"
            | "replace" | "replaceAll" => TriggerClass::InCycle,
            // `Array.from(iterable, mapFn)`: the map callback runs
            // synchronously. Receiver-restricted — a bare `.from` on an
            // unknown object could be anything.
            "from" if matches!(obj.as_ref(), Expr::Var(v) if v == "Array") => {
                TriggerClass::InCycle
            }
            "addEventListener" | "removeEventListener" => TriggerClass::Subscription,
            _ => TriggerClass::Unknown,
        },
        _ => TriggerClass::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::StateValue;

    fn env() -> AbstractEnv<StateValue> {
        AbstractEnv::new()
    }

    fn method(obj: &str, field: &str) -> Expr {
        Expr::FieldAccess {
            obj: Box::new(Expr::Var(obj.to_string())),
            field: field.to_string(),
        }
    }

    #[test]
    fn sync_array_hofs_are_in_cycle() {
        for f in [
            "map",
            "forEach",
            "sort",
            "toSorted",
            "findIndex",
            "findLast",
            "findLastIndex",
            "reduceRight",
        ] {
            assert_eq!(
                classify_callee(&method("arr", f), &env()),
                TriggerClass::InCycle,
                "{f} must be in-cycle"
            );
        }
    }

    #[test]
    fn string_replacer_callbacks_are_in_cycle() {
        assert_eq!(
            classify_callee(&method("s", "replace"), &env()),
            TriggerClass::InCycle
        );
        assert_eq!(
            classify_callee(&method("s", "replaceAll"), &env()),
            TriggerClass::InCycle
        );
    }

    #[test]
    fn array_from_is_receiver_restricted() {
        assert_eq!(
            classify_callee(&method("Array", "from"), &env()),
            TriggerClass::InCycle
        );
        // `.from` on an unknown receiver could be anything (e.g. a query
        // builder) — stays Unknown, FP-averse.
        assert_eq!(
            classify_callee(&method("router", "from"), &env()),
            TriggerClass::Unknown
        );
    }

    #[test]
    fn subscriptions_and_unknowns_unchanged() {
        assert_eq!(
            classify_callee(&method("el", "addEventListener"), &env()),
            TriggerClass::Subscription
        );
        assert_eq!(
            classify_callee(&method("api", "subscribe"), &env()),
            TriggerClass::Unknown
        );
    }
}
