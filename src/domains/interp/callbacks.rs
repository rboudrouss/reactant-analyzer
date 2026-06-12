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
        Expr::FieldAccess { field, .. } => match field.as_str() {
            "then" | "catch" | "finally" | "allSettled" | "any" => TriggerClass::InCycle,
            "map" | "forEach" | "reduce" | "filter" | "find" | "flatMap" | "some" | "every" => {
                TriggerClass::InCycle
            }
            "addEventListener" | "removeEventListener" => TriggerClass::Subscription,
            _ => TriggerClass::Unknown,
        },
        _ => TriggerClass::Unknown,
    }
}
