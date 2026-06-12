use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::ir::expr::{Expr, Prim};
use crate::ir::types::{HookLabel, Symbol};

use super::Stability;
pub use super::bool_val::BoolVal;
pub use super::interval::Interval;

/// Max strings tracked in a `StrConst` set before widening to `Str`.
pub(crate) const STR_WIDEN_THRESHOLD: usize = 4;

pub(crate) fn str_const(set: BTreeSet<String>) -> StateValue {
    if set.len() > STR_WIDEN_THRESHOLD {
        StateValue::Str
    } else {
        StateValue::StrConst(Arc::new(set))
    }
}

// ── StateValue ────────────────────────────────────────────────────────────────

/// Rich abstract value for useState state labels.
///
/// Unlike `Stability` (height-2 lattice), `StateValue` tracks concrete
/// numeric ranges via `Interval`, string constants via powerset, enabling
/// proper widening and infinite-loop detection.
#[derive(Debug, Clone, PartialEq)]
pub enum StateValue {
    /// ⊥ unreachable / not yet set.
    Bottom,
    /// JS `null`.
    Null,
    /// JS `undefined`.
    Undefined,
    /// Numeric value in interval [lo, hi].
    Number(Interval),
    /// Boolean value.
    Boolean(BoolVal),
    /// Known finite set of string constants. Widens to `Str` when |set| > STR_WIDEN_THRESHOLD.
    StrConst(Arc<BTreeSet<String>>),
    /// String with unknown content (string-type ⊤).
    Str,
    /// Object / array / function track reference stability.
    Reference(Stability),
    /// A specific `useState` setter from another component.
    /// React guarantees setter identity across renders → always `Stable`.
    ComponentSetter { component: Symbol, label: HookLabel },
    /// ⊤ any JS value, precision lost.
    Top,
}

impl StateValue {
    /// Derive the best initial abstract value from a `useState(init)` expression.
    pub fn from_init(init: &Expr) -> Self {
        match init {
            Expr::Lit(Prim::Int(n)) => StateValue::Number(Interval::point(*n as f64)),
            Expr::Lit(Prim::Float(f)) => StateValue::Number(Interval::point(*f)),
            Expr::Lit(Prim::Bool(b)) => {
                StateValue::Boolean(if *b { BoolVal::True } else { BoolVal::False })
            }
            Expr::Lit(Prim::String(s)) => str_const(std::iter::once(s.to_string()).collect()),
            Expr::Lit(Prim::Null) => StateValue::Null,
            Expr::Lit(Prim::Unit) => StateValue::Undefined,
            Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::FnLit { .. } => {
                StateValue::Reference(Stability::Unstable)
            }
            _ => StateValue::Top,
        }
    }

    /// Derive a `Stability` approximation from this value.
    ///
    /// Used by rules and `recompute_memo` that still reason in stability terms.
    pub fn to_stability(&self) -> Stability {
        match self {
            StateValue::Bottom => Stability::Bottom,
            StateValue::Null | StateValue::Undefined => Stability::Stable,
            StateValue::Number(i) if i.is_point() => Stability::Stable,
            StateValue::Number(_) => Stability::Unstable,
            StateValue::Boolean(BoolVal::Top | BoolVal::Bottom) => Stability::Unknown,
            StateValue::Boolean(_) => Stability::Stable,
            StateValue::StrConst(set) if set.len() == 1 => Stability::Stable,
            StateValue::StrConst(_) => Stability::Unknown,
            StateValue::Str => Stability::Unknown,
            StateValue::Reference(s) => *s,
            StateValue::ComponentSetter { .. } => Stability::Stable,
            StateValue::Top => Stability::Unknown,
        }
    }

    /// True if this value represents unbounded growth in the fixpoint.
    ///
    /// Used by `InfiniteLoop` to distinguish a setter that writes a bounded value
    /// (branch narrowing held the growth) from one that truly diverges:
    /// - Infinite interval bounds → numeric counter without a binding guard
    /// - `Reference(Unstable)` → new object/function literal every render
    /// - `Top` → precision lost, conservatively unbounded
    pub fn is_unbounded(&self) -> bool {
        match self {
            StateValue::Number(i) => i.lo.is_infinite() || i.hi.is_infinite(),
            StateValue::Reference(Stability::Unstable) => true,
            StateValue::Top => true,
            _ => false,
        }
    }

    /// True if this value is definitively stable (won't cause a re-render).
    pub fn is_stable(&self) -> bool {
        matches!(self.to_stability(), Stability::Stable)
    }

    /// True if this value is definitively unstable (always causes re-render).
    pub fn is_unstable(&self) -> bool {
        matches!(self.to_stability(), Stability::Unstable)
    }
}

impl PartialOrd for StateValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (StateValue::Bottom, _) => Some(Ordering::Less),
            (_, StateValue::Bottom) => Some(Ordering::Greater),
            (_, StateValue::Top) => Some(Ordering::Less),
            (StateValue::Top, _) => Some(Ordering::Greater),
            (StateValue::Number(a), StateValue::Number(b)) => a.partial_cmp(b),
            (StateValue::Boolean(a), StateValue::Boolean(b)) => a.partial_cmp(b),
            (StateValue::Reference(a), StateValue::Reference(b)) => a.partial_cmp(b),
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                if a == b {
                    Some(Ordering::Equal)
                } else if a.is_subset(b) {
                    Some(Ordering::Less)
                } else if b.is_subset(a) {
                    Some(Ordering::Greater)
                } else {
                    None
                }
            }
            (StateValue::StrConst(_), StateValue::Str) => Some(Ordering::Less),
            (StateValue::Str, StateValue::StrConst(_)) => Some(Ordering::Greater),
            (StateValue::Null, StateValue::Null) => Some(Ordering::Equal),
            (StateValue::Undefined, StateValue::Undefined) => Some(Ordering::Equal),
            (StateValue::Str, StateValue::Str) => Some(Ordering::Equal),
            // ComponentSetter ⊑ Reference(Stable) ⊑ Reference(Unknown)
            (
                StateValue::ComponentSetter { .. },
                StateValue::Reference(Stability::Stable | Stability::Unknown),
            ) => Some(Ordering::Less),
            (
                StateValue::Reference(Stability::Stable | Stability::Unknown),
                StateValue::ComponentSetter { .. },
            ) => Some(Ordering::Greater),
            _ => None,
        }
    }
}

use crate::domains::AbstractDomain;

impl AbstractDomain for StateValue {
    fn bottom() -> Self {
        StateValue::Bottom
    }
    fn top() -> Self {
        StateValue::Top
    }
    fn is_bottom(&self) -> bool {
        matches!(self, StateValue::Bottom)
    }

    fn as_state_value(&self) -> Option<StateValue> {
        Some(self.clone())
    }

    fn from_state_value(sv: StateValue) -> Self {
        sv
    }

    fn narrow_lt(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_lt(v)),
            _ => self,
        }
    }
    fn narrow_leq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_leq(v)),
            _ => self,
        }
    }
    fn narrow_gt(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_gt(v)),
            _ => self,
        }
    }
    fn narrow_geq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_geq(v)),
            _ => self,
        }
    }
    fn narrow_eq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_eq(v)),
            _ => self,
        }
    }
    fn narrow_neq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_neq(v)),
            _ => self,
        }
    }

    fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StateValue::Bottom, x) | (x, StateValue::Bottom) => x.clone(),
            (StateValue::Top, _) | (_, StateValue::Top) => StateValue::Top,
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.hull(b)),
            (StateValue::Boolean(a), StateValue::Boolean(b)) => StateValue::Boolean(a.join(b)),
            (StateValue::Reference(a), StateValue::Reference(b)) => {
                StateValue::Reference(a.join(b))
            }
            (StateValue::Null, StateValue::Null) => StateValue::Null,
            (StateValue::Undefined, StateValue::Undefined) => StateValue::Undefined,
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                str_const(a.iter().cloned().chain(b.iter().cloned()).collect())
            }
            (StateValue::StrConst(_), StateValue::Str)
            | (StateValue::Str, StateValue::StrConst(_))
            | (StateValue::Str, StateValue::Str) => StateValue::Str,
            // Different ComponentSetters are both stable references
            (StateValue::ComponentSetter { .. }, StateValue::ComponentSetter { .. }) => {
                StateValue::Reference(Stability::Stable)
            }
            // ComponentSetter joined with a Reference: join stabilities
            (StateValue::ComponentSetter { .. }, StateValue::Reference(s))
            | (StateValue::Reference(s), StateValue::ComponentSetter { .. }) => {
                StateValue::Reference(s.join(&Stability::Stable))
            }
            _ => StateValue::Top,
        }
    }

    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StateValue::Top, x) | (x, StateValue::Top) => x.clone(),
            (StateValue::Bottom, _) | (_, StateValue::Bottom) => StateValue::Bottom,
            (StateValue::Number(a), StateValue::Number(b)) => {
                let hull = a.hull(b);
                if hull == *a && hull == *b {
                    StateValue::Number(*a)
                } else {
                    StateValue::Bottom
                }
            }
            (StateValue::Boolean(a), StateValue::Boolean(b)) => StateValue::Boolean(a.meet(b)),
            (StateValue::Reference(a), StateValue::Reference(b)) => {
                StateValue::Reference(a.meet(b))
            }
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                let inter: BTreeSet<String> = a.intersection(b).cloned().collect();
                if inter.is_empty() {
                    StateValue::Bottom
                } else {
                    StateValue::StrConst(Arc::new(inter))
                }
            }
            (StateValue::StrConst(a), StateValue::Str)
            | (StateValue::Str, StateValue::StrConst(a)) => StateValue::StrConst(a.clone()),
            // ComponentSetter ⊑ Reference(Stable|Unknown): meet = ComponentSetter
            (
                cs @ StateValue::ComponentSetter { .. },
                StateValue::Reference(Stability::Stable | Stability::Unknown),
            ) => cs.clone(),
            (
                StateValue::Reference(Stability::Stable | Stability::Unknown),
                cs @ StateValue::ComponentSetter { .. },
            ) => cs.clone(),
            // Different ComponentSetters: incompatible
            (StateValue::ComponentSetter { .. }, StateValue::ComponentSetter { .. }) => {
                StateValue::Bottom
            }
            _ => StateValue::Bottom,
        }
    }

    fn widen(&self, other: &Self) -> Self {
        match (self, other) {
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.widen(b)),
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                str_const(a.iter().cloned().chain(b.iter().cloned()).collect())
            }
            _ => self.join(other),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::Prim;

    fn str_singleton(s: &str) -> StateValue {
        StateValue::StrConst(Arc::new(std::iter::once(s.to_string()).collect()))
    }
    fn str_pair(a: &str, b: &str) -> StateValue {
        StateValue::StrConst(Arc::new(
            [a.to_string(), b.to_string()].into_iter().collect(),
        ))
    }

    #[test]
    fn bottom_is_least() {
        assert!(StateValue::Bottom <= StateValue::Number(Interval::point(0.0)));
        assert!(StateValue::Bottom <= StateValue::Reference(Stability::Stable));
        assert!(StateValue::Bottom <= StateValue::Top);
    }

    #[test]
    fn top_is_greatest() {
        assert!(StateValue::Number(Interval::point(0.0)) <= StateValue::Top);
        assert!(StateValue::Reference(Stability::Unstable) <= StateValue::Top);
    }

    #[test]
    fn number_join_is_hull() {
        let a = StateValue::Number(Interval::point(0.0));
        let b = StateValue::Number(Interval::point(1.0));
        assert_eq!(
            a.join(&b),
            StateValue::Number(Interval { lo: 0.0, hi: 1.0 })
        );
    }

    #[test]
    fn cross_type_join_is_top() {
        let n = StateValue::Number(Interval::point(0.0));
        let r = StateValue::Reference(Stability::Stable);
        assert_eq!(n.join(&r), StateValue::Top);
    }

    #[test]
    fn number_widen_grows_bound() {
        let a = StateValue::Number(Interval::point(0.0));
        let b = StateValue::Number(Interval::point(1.0));
        let w = a.widen(&b);
        match w {
            StateValue::Number(i) => {
                assert_eq!(i.lo, 0.0);
                assert!(i.hi.is_infinite());
            }
            _ => panic!("expected Number after widen"),
        }
    }

    #[test]
    fn to_stability_point_is_stable() {
        assert!(StateValue::Number(Interval::point(42.0)).is_stable());
        assert!(StateValue::Null.is_stable());
        assert!(StateValue::Boolean(BoolVal::True).is_stable());
    }

    #[test]
    fn to_stability_wide_interval_is_unstable() {
        assert!(StateValue::Number(Interval { lo: 0.0, hi: 5.0 }).is_unstable());
        assert!(StateValue::Reference(Stability::Unstable).is_unstable());
    }

    #[test]
    fn from_init_int_gives_point_interval() {
        assert_eq!(
            StateValue::from_init(&Expr::Lit(Prim::Int(0))),
            StateValue::Number(Interval::point(0.0))
        );
    }

    #[test]
    fn from_init_null_gives_null() {
        assert_eq!(
            StateValue::from_init(&Expr::Lit(Prim::Null)),
            StateValue::Null
        );
    }

    #[test]
    fn from_init_object_gives_unstable_reference() {
        assert_eq!(
            StateValue::from_init(&Expr::ObjectLit {
                id: crate::ir::types::ExprId(0),
                fields: vec![],
            }),
            StateValue::Reference(Stability::Unstable)
        );
    }

    #[test]
    fn from_init_string_gives_singleton() {
        let v = StateValue::from_init(&Expr::Lit(Prim::String("hello".into())));
        assert_eq!(v, str_singleton("hello"));
        assert!(v.is_stable());
    }

    // ── Narrowing ─────────────────────────────────────────────────────────────

    #[test]
    fn interval_narrow_lt_caps_hi() {
        let i = Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        };
        let n = i.narrow_lt(10.0);
        assert_eq!(n.lo, 0.0);
        assert_eq!(n.hi, 9.0);
    }

    #[test]
    fn interval_narrow_geq_lifts_lo() {
        let i = Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        };
        let n = i.narrow_geq(10.0);
        assert_eq!(n.lo, 10.0);
        assert!(n.hi.is_infinite());
    }

    #[test]
    fn interval_narrow_eq_in_range_gives_point() {
        let i = Interval { lo: 0.0, hi: 5.0 };
        assert_eq!(i.narrow_eq(3.0), Interval::point(3.0));
    }

    #[test]
    fn interval_narrow_eq_out_of_range_gives_bottom() {
        let i = Interval { lo: 0.0, hi: 5.0 };
        assert!(i.narrow_eq(7.0).is_bottom());
    }

    #[test]
    fn state_value_narrow_lt_on_number() {
        let v = StateValue::Number(Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        });
        let n = v.narrow_lt(10.0);
        assert_eq!(n, StateValue::Number(Interval { lo: 0.0, hi: 9.0 }));
    }

    #[test]
    fn state_value_narrow_non_number_identity() {
        assert_eq!(StateValue::Null.narrow_lt(5.0), StateValue::Null);
        assert_eq!(StateValue::Top.narrow_geq(0.0), StateValue::Top);
    }

    // ── StrConst ──────────────────────────────────────────────────────────────

    #[test]
    fn str_singleton_is_stable() {
        assert!(str_singleton("dark").is_stable());
    }

    #[test]
    fn str_multi_is_not_stable() {
        assert!(!str_pair("light", "dark").is_stable());
    }

    #[test]
    fn str_join_same_singleton_idempotent() {
        let a = str_singleton("x");
        assert_eq!(a.join(&str_singleton("x")), str_singleton("x"));
    }

    #[test]
    fn str_join_two_singletons_gives_pair() {
        let j = str_singleton("light").join(&str_singleton("dark"));
        assert_eq!(j, str_pair("dark", "light"));
    }

    #[test]
    fn str_join_with_str_top_gives_str() {
        let j = str_singleton("x").join(&StateValue::Str);
        assert_eq!(j, StateValue::Str);
    }

    #[test]
    fn str_join_beyond_threshold_widens_to_str() {
        let mut v = str_singleton("a");
        for c in ["b", "c", "d", "e"] {
            v = v.join(&str_singleton(c));
        }
        assert_eq!(v, StateValue::Str);
    }

    #[test]
    fn str_partial_ord_subset() {
        let single = str_singleton("a");
        let pair = str_pair("a", "b");
        assert!(single < pair);
        assert!(!(pair < single));
        assert!(single <= StateValue::Str);
    }

    #[test]
    fn str_meet_gives_intersection() {
        let a = str_pair("x", "y");
        let b = str_pair("y", "z");
        let m = a.meet(&b);
        assert_eq!(m, str_singleton("y"));
    }

    // ── ComponentSetter ───────────────────────────────────────────────────────

    fn cs(comp: &str, label: usize) -> StateValue {
        StateValue::ComponentSetter {
            component: comp.to_string(),
            label,
        }
    }

    #[test]
    fn component_setter_is_stable() {
        assert!(cs("Foo", 0).is_stable());
        assert!(!cs("Foo", 0).is_unstable());
        assert!(!cs("Foo", 0).is_unbounded());
        assert!(!cs("Foo", 0).is_bottom());
    }

    #[test]
    fn component_setter_to_stability_is_stable() {
        assert_eq!(cs("Foo", 0).to_stability(), Stability::Stable);
    }

    #[test]
    fn component_setter_join_same_is_identity() {
        assert_eq!(cs("Foo", 0).join(&cs("Foo", 0)), cs("Foo", 0));
    }

    #[test]
    fn component_setter_join_different_comp_gives_ref_stable() {
        let j = cs("Foo", 0).join(&cs("Bar", 0));
        assert_eq!(j, StateValue::Reference(Stability::Stable));
    }

    #[test]
    fn component_setter_join_different_label_gives_ref_stable() {
        let j = cs("Foo", 0).join(&cs("Foo", 1));
        assert_eq!(j, StateValue::Reference(Stability::Stable));
    }

    #[test]
    fn component_setter_join_with_ref_stable_is_ref_stable() {
        let j = cs("Foo", 0).join(&StateValue::Reference(Stability::Stable));
        assert_eq!(j, StateValue::Reference(Stability::Stable));
    }

    #[test]
    fn component_setter_join_with_ref_unstable_gives_unknown() {
        let j = cs("Foo", 0).join(&StateValue::Reference(Stability::Unstable));
        assert_eq!(j, StateValue::Reference(Stability::Unknown));
    }

    #[test]
    fn component_setter_join_with_top_gives_top() {
        assert_eq!(cs("Foo", 0).join(&StateValue::Top), StateValue::Top);
    }

    #[test]
    fn component_setter_join_with_bottom_gives_self() {
        assert_eq!(cs("Foo", 0).join(&StateValue::Bottom), cs("Foo", 0));
    }

    #[test]
    fn component_setter_leq_ref_stable() {
        assert!(cs("Foo", 0) <= StateValue::Reference(Stability::Stable));
        assert!(cs("Foo", 0) <= StateValue::Reference(Stability::Unknown));
        assert!(cs("Foo", 0) <= StateValue::Top);
        assert!(StateValue::Bottom <= cs("Foo", 0));
    }

    #[test]
    fn component_setter_not_leq_ref_unstable() {
        use std::cmp::Ordering;
        let cmp = cs("Foo", 0).partial_cmp(&StateValue::Reference(Stability::Unstable));
        assert!(
            cmp.is_none(),
            "ComponentSetter vs Unstable should be incomparable"
        );
    }

    #[test]
    fn different_component_setters_incomparable() {
        let cmp = cs("Foo", 0).partial_cmp(&cs("Bar", 0));
        assert!(cmp.is_none());
    }

    #[test]
    fn component_setter_meet_same_is_identity() {
        assert_eq!(cs("Foo", 0).meet(&cs("Foo", 0)), cs("Foo", 0));
    }

    #[test]
    fn component_setter_meet_different_is_bottom() {
        assert_eq!(cs("Foo", 0).meet(&cs("Bar", 0)), StateValue::Bottom);
    }

    #[test]
    fn component_setter_meet_ref_stable_is_setter() {
        assert_eq!(
            cs("Foo", 0).meet(&StateValue::Reference(Stability::Stable)),
            cs("Foo", 0)
        );
        assert_eq!(
            StateValue::Reference(Stability::Stable).meet(&cs("Foo", 0)),
            cs("Foo", 0)
        );
    }

    #[test]
    fn component_setter_widen_same_is_identity() {
        assert_eq!(cs("Foo", 0).widen(&cs("Foo", 0)), cs("Foo", 0));
    }

    #[test]
    fn component_setter_as_state_value_roundtrip() {
        use crate::domains::AbstractDomain;
        let sv = cs("Foo", 0);
        assert_eq!(sv.as_state_value(), Some(sv.clone()));
        assert_eq!(StateValue::from_state_value(sv.clone()), sv);
    }
}
