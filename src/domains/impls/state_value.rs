use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use crate::ir::expr::{Expr, Prim};
use crate::ir::types::{HookLabel, Symbol};

use super::Stability;
pub use super::bool_val::BoolVal;
pub use super::interval::Interval;
pub use super::setter_val::SetterVal;
pub use super::str_const::StrConst;

// ── StateValue ────────────────────────────────────────────────────────────────

/// Abstract JS value: pointwise product over the disjoint JS kinds (ADR-015).
///
/// JS primitive kinds are mutually exclusive (a value is never a number AND a
/// string), so the disjunctive completion of the kind sum degenerates into a
/// product: one independent slot per kind, each ⊥ when that kind is impossible.
/// `join`/`meet`/`widen` are pointwise — a cross-kind join keeps BOTH kinds
/// (`number | null`, `number | string`) instead of collapsing to ⊤, which is
/// what enables infinite-loop detection through nullable states without a
/// TypeScript hint.
#[derive(Clone, PartialEq)]
pub struct StateValue {
    /// Numeric kind — interval [lo, hi]; ⊥ = cannot be a number.
    pub num: Interval,
    /// Boolean kind.
    pub boolean: BoolVal,
    /// String kind — finite constant set, threshold-widened to ⊤.
    pub str: StrConst,
    /// Object/array/function kind — reference stability; ⊥ = not a reference.
    pub reference: Stability,
    /// `null` possible?
    pub null: bool,
    /// `undefined` possible?
    pub undef: bool,
    /// Cross-component useState setter (flat lattice with identity payload).
    pub setter: SetterVal,
    /// Residual ⊤ — kinds not modelled (symbol, bigint, …). `true` means the
    /// value may be something outside every other slot.
    pub other: bool,
}

impl StateValue {
    // ── Constructors (one per kind, mirroring the old enum variants) ──────────

    pub fn number(i: Interval) -> Self {
        StateValue {
            num: i,
            ..Self::bottom_value()
        }
    }

    pub fn boolean(b: BoolVal) -> Self {
        StateValue {
            boolean: b,
            ..Self::bottom_value()
        }
    }

    /// String slot from a constant set (threshold-widened by `StrConst`).
    pub fn str_set(set: BTreeSet<String>) -> Self {
        StateValue {
            str: StrConst::from_set(set),
            ..Self::bottom_value()
        }
    }

    pub fn str_singleton(s: String) -> Self {
        StateValue {
            str: StrConst::singleton(s),
            ..Self::bottom_value()
        }
    }

    /// String of unknown content (string-kind ⊤).
    pub fn str_top() -> Self {
        StateValue {
            str: StrConst::Top,
            ..Self::bottom_value()
        }
    }

    pub fn reference(s: Stability) -> Self {
        StateValue {
            reference: s,
            ..Self::bottom_value()
        }
    }

    pub fn null() -> Self {
        StateValue {
            null: true,
            ..Self::bottom_value()
        }
    }

    pub fn undefined() -> Self {
        StateValue {
            undef: true,
            ..Self::bottom_value()
        }
    }

    pub fn component_setter(component: Symbol, label: HookLabel) -> Self {
        StateValue {
            setter: SetterVal::One(component, label),
            ..Self::bottom_value()
        }
    }

    /// `const` version of `AbstractDomain::bottom()` usable in struct-update syntax.
    const fn bottom_value() -> Self {
        StateValue {
            num: Interval {
                lo: f64::INFINITY,
                hi: f64::NEG_INFINITY,
            },
            boolean: BoolVal::Bottom,
            str: StrConst::Bottom,
            reference: Stability::Bottom,
            null: false,
            undef: false,
            setter: SetterVal::Bottom,
            other: false,
        }
    }

    // ── Slot accessors ─────────────────────────────────────────────────────────

    /// True when every slot is ⊥ (unreachable / not yet set).
    pub fn is_bottom_value(&self) -> bool {
        self.num.is_bottom()
            && self.boolean == BoolVal::Bottom
            && self.str == StrConst::Bottom
            && self.reference == Stability::Bottom
            && !self.null
            && !self.undef
            && self.setter == SetterVal::Bottom
            && !self.other
    }

    /// True when every slot is ⊤ — any JS value, precision lost.
    pub fn is_top_value(&self) -> bool {
        self.num.is_top()
            && self.boolean == BoolVal::Top
            && self.str == StrConst::Top
            && self.reference == Stability::Unknown
            && self.null
            && self.undef
            && self.setter == SetterVal::Top
            && self.other
    }

    /// Exact setter identity, only when the value can be nothing else.
    /// Mirrors the old exact `ComponentSetter { .. }` match.
    pub fn as_setter(&self) -> Option<(&Symbol, &HookLabel)> {
        if self.num.is_bottom()
            && self.boolean == BoolVal::Bottom
            && self.str == StrConst::Bottom
            && self.reference == Stability::Bottom
            && !self.null
            && !self.undef
            && !self.other
        {
            self.setter.as_one()
        } else {
            None
        }
    }

    /// True when the value is exactly an unstable reference (nothing else).
    /// Mirrors the old exact `Reference(Stability::Unstable)` match.
    pub fn is_unstable_reference_only(&self) -> bool {
        self.reference == Stability::Unstable
            && self.num.is_bottom()
            && self.boolean == BoolVal::Bottom
            && self.str == StrConst::Bottom
            && !self.null
            && !self.undef
            && self.setter == SetterVal::Bottom
            && !self.other
    }

    /// Derive the best initial abstract value from a `useState(init)` expression.
    pub fn from_init(init: &Expr) -> Self {
        match init {
            Expr::Lit(Prim::Int(n)) => StateValue::number(Interval::point(*n as f64)),
            Expr::Lit(Prim::Float(f)) => StateValue::number(Interval::point(*f)),
            Expr::Lit(Prim::Bool(b)) => {
                StateValue::boolean(if *b { BoolVal::True } else { BoolVal::False })
            }
            Expr::Lit(Prim::String(s)) => StateValue::str_singleton(s.to_string()),
            Expr::Lit(Prim::Null) => StateValue::null(),
            Expr::Lit(Prim::Unit) => StateValue::undefined(),
            Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::FnLit { .. } => {
                StateValue::reference(Stability::Unstable)
            }
            _ => StateValue::top(),
        }
    }

    /// Derive a `Stability` approximation from this value.
    ///
    /// Per-kind mapping (same as the pre-ADR-015 flat lattice), combined with
    /// *motion-wins* priority: a slot known to be in motion across renders
    /// (non-point number interval, unstable reference) makes the whole value
    /// `Unstable` even if another slot is individually stable — a state that
    /// widened through `null ∪ number[0,+∞)` genuinely changes every render.
    /// The residual `other` slot forces `Unknown` (an opaque value must never
    /// claim definite (in)stability — Top stayed Unknown before ADR-015 too).
    ///
    /// Used by rules and `recompute_memo` that still reason in stability terms.
    pub fn to_stability(&self) -> Stability {
        if self.other {
            return Stability::Unknown;
        }
        let mut acc = Stability::Bottom;
        let mut in_motion = false;
        if !self.num.is_bottom() {
            if self.num.is_point() {
                acc = acc.join(&Stability::Stable);
            } else {
                in_motion = true;
            }
        }
        match self.boolean {
            BoolVal::Bottom => {}
            BoolVal::True | BoolVal::False => acc = acc.join(&Stability::Stable),
            BoolVal::Top => acc = acc.join(&Stability::Unknown),
        }
        match &self.str {
            StrConst::Bottom => {}
            StrConst::Set(set) if set.len() == 1 => acc = acc.join(&Stability::Stable),
            _ => acc = acc.join(&Stability::Unknown),
        }
        match self.reference {
            Stability::Unstable => in_motion = true,
            s => acc = acc.join(&s),
        }
        if self.null || self.undef {
            acc = acc.join(&Stability::Stable);
        }
        if self.setter != SetterVal::Bottom {
            // React guarantees setter identity across renders.
            acc = acc.join(&Stability::Stable);
        }
        if in_motion {
            return Stability::Unstable;
        }
        acc
    }

    /// True if this value represents unbounded growth in the fixpoint.
    ///
    /// Used by `InfiniteLoop` to distinguish a setter that writes a bounded value
    /// (branch narrowing held the growth) from one that truly diverges:
    /// - Infinite interval bounds → numeric counter without a binding guard
    /// - Unstable reference slot → new object/function literal every render
    /// - Residual ⊤ → precision lost, conservatively unbounded
    pub fn is_unbounded(&self) -> bool {
        (!self.num.is_bottom() && (self.num.lo.is_infinite() || self.num.hi.is_infinite()))
            || self.reference == Stability::Unstable
            || self.other
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

// ── Debug — concise kind-union rendering (used in diagnostics via {:?}) ───────

impl fmt::Debug for StateValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_bottom_value() {
            return write!(f, "⊥");
        }
        if self.is_top_value() {
            return write!(f, "⊤");
        }
        let mut parts: Vec<String> = vec![];
        if !self.num.is_bottom() {
            if self.num.is_top() {
                parts.push("number".into());
            } else {
                parts.push(format!("number[{}, {}]", self.num.lo, self.num.hi));
            }
        }
        match self.boolean {
            BoolVal::Bottom => {}
            BoolVal::True => parts.push("true".into()),
            BoolVal::False => parts.push("false".into()),
            BoolVal::Top => parts.push("boolean".into()),
        }
        match &self.str {
            StrConst::Bottom => {}
            StrConst::Set(set) => parts.push(format!(
                "string{{{}}}",
                set.iter()
                    .map(|s| format!("{s:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            StrConst::Top => parts.push("string".into()),
        }
        match self.reference {
            Stability::Bottom => {}
            s => parts.push(format!("ref({s:?})")),
        }
        if self.null {
            parts.push("null".into());
        }
        if self.undef {
            parts.push("undefined".into());
        }
        match &self.setter {
            SetterVal::Bottom => {}
            SetterVal::One(c, l) => parts.push(format!("setter({c}#{l})")),
            SetterVal::Top => parts.push("setter".into()),
        }
        if self.other {
            parts.push("other".into());
        }
        write!(f, "{}", parts.join("|"))
    }
}

// ── Lattice order — pointwise ─────────────────────────────────────────────────

/// Combine two per-slot orderings; `None` when slots disagree in direction.
fn merge_ord(acc: Option<Ordering>, next: Option<Ordering>) -> Option<Ordering> {
    match (acc?, next?) {
        (Ordering::Equal, x) | (x, Ordering::Equal) => Some(x),
        (Ordering::Less, Ordering::Less) => Some(Ordering::Less),
        (Ordering::Greater, Ordering::Greater) => Some(Ordering::Greater),
        _ => None,
    }
}

impl PartialOrd for StateValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let mut ord = self.num.partial_cmp(&other.num);
        ord = merge_ord(ord, self.boolean.partial_cmp(&other.boolean));
        ord = merge_ord(ord, self.str.partial_cmp(&other.str));
        ord = merge_ord(ord, self.reference.partial_cmp(&other.reference));
        ord = merge_ord(ord, self.null.partial_cmp(&other.null));
        ord = merge_ord(ord, self.undef.partial_cmp(&other.undef));
        ord = merge_ord(ord, self.setter.partial_cmp(&other.setter));
        ord = merge_ord(ord, self.other.partial_cmp(&other.other));
        ord
    }
}

// ── AbstractDomain — pointwise ────────────────────────────────────────────────

use crate::domains::AbstractDomain;

impl AbstractDomain for StateValue {
    fn bottom() -> Self {
        Self::bottom_value()
    }

    fn top() -> Self {
        StateValue {
            num: Interval::top(),
            boolean: BoolVal::Top,
            str: StrConst::Top,
            reference: Stability::Unknown,
            null: true,
            undef: true,
            setter: SetterVal::Top,
            other: true,
        }
    }

    fn is_bottom(&self) -> bool {
        self.is_bottom_value()
    }

    fn as_state_value(&self) -> Option<StateValue> {
        Some(self.clone())
    }

    fn from_state_value(sv: StateValue) -> Self {
        sv
    }

    // ── Nullability narrowing (branch guards on null/undefined) ───────────────

    /// Taken `x !== null` guard: the null slot dies.
    fn narrow_drop_null(mut self) -> Self {
        self.null = false;
        self
    }

    /// Taken `x !== undefined` guard: the undef slot dies.
    fn narrow_drop_undef(mut self) -> Self {
        self.undef = false;
        self
    }

    /// Taken `x == null` guard: only null/undefined survive (`other` may hide
    /// either, so it conservatively re-enables both).
    fn narrow_keep_nullish_only(self) -> Self {
        StateValue {
            null: self.null || self.other,
            undef: self.undef || self.other,
            ..Self::bottom_value()
        }
    }

    /// Taken truthiness guard `if (x)`: excludes every falsy JS value —
    /// null, undefined, 0, "" and false. References/setters are always
    /// truthy → unchanged; `other` may hide truthy kinds → kept.
    /// (NaN is falsy too but intervals never claim to contain it.)
    fn narrow_truthy(mut self) -> Self {
        self.null = false;
        self.undef = false;
        self.num = self.num.narrow_neq(0.0);
        self.boolean = match self.boolean {
            BoolVal::False => BoolVal::Bottom,
            BoolVal::Top => BoolVal::True,
            b => b,
        };
        if let StrConst::Set(set) = &self.str {
            if set.contains("") {
                let filtered: BTreeSet<String> =
                    set.iter().filter(|s| !s.is_empty()).cloned().collect();
                self.str = StrConst::from_set(filtered); // empty set → ⊥
            }
        }
        self
    }

    /// Falsy branch (`else` of `if (x)`, taken `if (!x)`): only falsy values
    /// survive — null, undefined, 0, "" and false. References and setters
    /// are always truthy → ⊥.
    fn narrow_falsy(mut self) -> Self {
        self.num = self.num.narrow_eq(0.0);
        self.boolean = match self.boolean {
            BoolVal::True => BoolVal::Bottom,
            BoolVal::Top => BoolVal::False,
            b => b,
        };
        self.str = match &self.str {
            StrConst::Set(set) if set.contains("") => StrConst::singleton(String::new()),
            StrConst::Top => StrConst::singleton(String::new()),
            _ => StrConst::Bottom,
        };
        self.reference = Stability::Bottom;
        self.setter = SetterVal::Bottom;
        self
    }

    // Numeric narrowing acts on the num slot; other slots kept (sound:
    // JS coercion makes e.g. `null < 5` true, so they cannot be dropped).
    fn narrow_lt(mut self, v: f64) -> Self {
        self.num = self.num.narrow_lt(v);
        self
    }
    fn narrow_leq(mut self, v: f64) -> Self {
        self.num = self.num.narrow_leq(v);
        self
    }
    fn narrow_gt(mut self, v: f64) -> Self {
        self.num = self.num.narrow_gt(v);
        self
    }
    fn narrow_geq(mut self, v: f64) -> Self {
        self.num = self.num.narrow_geq(v);
        self
    }
    fn narrow_eq(mut self, v: f64) -> Self {
        self.num = self.num.narrow_eq(v);
        self
    }
    fn narrow_neq(mut self, v: f64) -> Self {
        self.num = self.num.narrow_neq(v);
        self
    }

    fn join(&self, other: &Self) -> Self {
        StateValue {
            num: self.num.hull(&other.num),
            boolean: self.boolean.join(&other.boolean),
            str: self.str.join(&other.str),
            reference: self.reference.join(&other.reference),
            null: self.null || other.null,
            undef: self.undef || other.undef,
            setter: self.setter.join(&other.setter),
            other: self.other || other.other,
        }
    }

    fn meet(&self, other: &Self) -> Self {
        StateValue {
            num: AbstractDomain::meet(&self.num, &other.num),
            boolean: self.boolean.meet(&other.boolean),
            str: AbstractDomain::meet(&self.str, &other.str),
            reference: self.reference.meet(&other.reference),
            null: self.null && other.null,
            undef: self.undef && other.undef,
            setter: AbstractDomain::meet(&self.setter, &other.setter),
            other: self.other && other.other,
        }
    }

    fn widen(&self, other: &Self) -> Self {
        StateValue {
            num: self.num.widen(&other.num),
            boolean: self.boolean.join(&other.boolean),
            str: self.str.widen(&other.str),
            reference: self.reference.join(&other.reference),
            null: self.null || other.null,
            undef: self.undef || other.undef,
            setter: self.setter.join(&other.setter),
            other: self.other || other.other,
        }
    }

    fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        StateValue {
            num: self.num.widen_to(&other.num, thresholds),
            // Non-numeric slots have no notion of a threshold → plain widen.
            ..self.widen(other)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::Prim;

    fn str_singleton(s: &str) -> StateValue {
        StateValue::str_singleton(s.to_string())
    }
    fn str_pair(a: &str, b: &str) -> StateValue {
        StateValue::str_set([a.to_string(), b.to_string()].into_iter().collect())
    }
    fn num_point(v: f64) -> StateValue {
        StateValue::number(Interval::point(v))
    }

    #[test]
    fn bottom_is_least() {
        assert!(StateValue::bottom() <= num_point(0.0));
        assert!(StateValue::bottom() <= StateValue::reference(Stability::Stable));
        assert!(StateValue::bottom() <= StateValue::top());
    }

    #[test]
    fn top_is_greatest() {
        assert!(num_point(0.0) <= StateValue::top());
        assert!(StateValue::reference(Stability::Unstable) <= StateValue::top());
        assert!(StateValue::null() <= StateValue::top());
    }

    #[test]
    fn number_join_is_hull() {
        let a = num_point(0.0);
        let b = num_point(1.0);
        assert_eq!(
            a.join(&b),
            StateValue::number(Interval { lo: 0.0, hi: 1.0 })
        );
    }

    #[test]
    fn cross_kind_join_keeps_both_slots() {
        // The whole point of ADR-015: join(number, ref) is NOT Top anymore.
        let n = num_point(0.0);
        let r = StateValue::reference(Stability::Stable);
        let j = n.join(&r);
        assert!(!j.is_top_value());
        assert_eq!(j.num, Interval::point(0.0));
        assert_eq!(j.reference, Stability::Stable);
        assert!(!j.null && !j.other);
    }

    #[test]
    fn null_number_join_keeps_interval() {
        // The ADR-008 residual FN case: join(Null, Number) used to be Top.
        let j = StateValue::null().join(&num_point(1.0));
        assert!(j.null);
        assert_eq!(j.num, Interval::point(1.0));
        assert!(!j.is_top_value());
        // and the num slot can keep widening:
        let w = j.widen(&StateValue::null().join(&num_point(2.0)));
        assert!(w.num.hi.is_infinite());
        assert!(w.null);
    }

    #[test]
    fn number_string_join_keeps_both() {
        // let a = 10; if (c) a = "test"  →  {number, string}, not Top.
        let j = num_point(10.0).join(&str_singleton("test"));
        assert_eq!(j.num, Interval::point(10.0));
        assert_eq!(j.str, StrConst::singleton("test".to_string()));
        assert!(!j.is_top_value());
    }

    #[test]
    fn number_widen_grows_bound() {
        let a = num_point(0.0);
        let b = num_point(1.0);
        let w = a.widen(&b);
        assert_eq!(w.num.lo, 0.0);
        assert!(w.num.hi.is_infinite());
    }

    #[test]
    fn to_stability_point_is_stable() {
        assert!(num_point(42.0).is_stable());
        assert!(StateValue::null().is_stable());
        assert!(StateValue::boolean(BoolVal::True).is_stable());
    }

    #[test]
    fn to_stability_wide_interval_is_unstable() {
        assert!(StateValue::number(Interval { lo: 0.0, hi: 5.0 }).is_unstable());
        assert!(StateValue::reference(Stability::Unstable).is_unstable());
    }

    #[test]
    fn to_stability_bottom_is_bottom() {
        assert_eq!(StateValue::bottom().to_stability(), Stability::Bottom);
    }

    #[test]
    fn to_stability_mixed_stable_kinds_is_stable() {
        // null ∪ point-number: both kinds individually stable → Stable.
        let v = StateValue::null().join(&num_point(1.0));
        assert_eq!(v.to_stability(), Stability::Stable);
    }

    #[test]
    fn is_unbounded_requires_active_num_slot() {
        // ⊥ interval has infinite sentinel bounds — must NOT count as unbounded.
        assert!(!StateValue::null().is_unbounded());
        assert!(!StateValue::bottom().is_unbounded());
        assert!(
            StateValue::number(Interval {
                lo: 0.0,
                hi: f64::INFINITY
            })
            .is_unbounded()
        );
        assert!(StateValue::top().is_unbounded());
    }

    #[test]
    fn from_init_int_gives_point_interval() {
        assert_eq!(
            StateValue::from_init(&Expr::Lit(Prim::Int(0))),
            num_point(0.0)
        );
    }

    #[test]
    fn from_init_null_gives_null() {
        assert_eq!(
            StateValue::from_init(&Expr::Lit(Prim::Null)),
            StateValue::null()
        );
    }

    #[test]
    fn from_init_object_gives_unstable_reference() {
        assert_eq!(
            StateValue::from_init(&Expr::ObjectLit {
                id: crate::ir::types::ExprId(0),
                fields: vec![],
            }),
            StateValue::reference(Stability::Unstable)
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
        let v = StateValue::number(Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        });
        let n = v.narrow_lt(10.0);
        assert_eq!(n, StateValue::number(Interval { lo: 0.0, hi: 9.0 }));
    }

    #[test]
    fn state_value_narrow_keeps_other_slots() {
        // {null, number[0,+∞)} narrowed by < 10 keeps null (JS coercion: null < 10).
        let v = StateValue::null().join(&StateValue::number(Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        }));
        let n = v.narrow_lt(10.0);
        assert!(n.null);
        assert_eq!(n.num, Interval { lo: 0.0, hi: 9.0 });
    }

    #[test]
    fn nullability_narrowing_drop_null() {
        let v = StateValue::null().join(&num_point(5.0));
        let n = v.narrow_drop_null();
        assert!(!n.null);
        assert_eq!(n.num, Interval::point(5.0));
    }

    #[test]
    fn nullability_narrowing_keep_nullish_only() {
        let v = StateValue::null().join(&num_point(5.0));
        let n = v.narrow_keep_nullish_only();
        assert_eq!(n, StateValue::null());
        // A value that cannot be nullish narrows to ⊥.
        assert!(num_point(5.0).narrow_keep_nullish_only().is_bottom());
        // `other` may hide null/undefined → conservatively kept.
        let t = StateValue::top().narrow_keep_nullish_only();
        assert!(t.null && t.undef && t.num.is_bottom());
    }

    #[test]
    fn nullability_narrowing_nullish() {
        let v = StateValue::null()
            .join(&StateValue::undefined())
            .join(&num_point(1.0));
        let truthy = v.clone().narrow_truthy();
        assert!(!truthy.null && !truthy.undef);
        assert_eq!(truthy.num, Interval::point(1.0));
        let kept = v.narrow_keep_nullish_only();
        assert!(kept.null && kept.undef);
        assert!(kept.num.is_bottom());
    }

    #[test]
    fn truthiness_narrowing_excludes_falsy_values() {
        // {null, 0, "", false, ref} — the truthy branch keeps only the ref.
        let v = StateValue::null()
            .join(&num_point(0.0))
            .join(&StateValue::str_singleton(String::new()))
            .join(&StateValue::boolean(BoolVal::False))
            .join(&StateValue::reference(Stability::Stable));
        let t = v.narrow_truthy();
        assert!(!t.null);
        assert!(t.num.is_bottom(), "point 0 is falsy → num slot dies");
        assert_eq!(t.str, StrConst::Bottom, "\"\" is falsy → str slot dies");
        assert_eq!(t.boolean, BoolVal::Bottom, "false is falsy");
        assert_eq!(t.reference, Stability::Stable, "references are truthy");
        // Wide interval and Top boolean survive (partially truthy).
        let w = StateValue::number(Interval { lo: 0.0, hi: 5.0 })
            .join(&StateValue::boolean(BoolVal::Top));
        let tw = w.narrow_truthy();
        assert_eq!(tw.num, Interval { lo: 0.0, hi: 5.0 }, "can't split [0,5]");
        assert_eq!(tw.boolean, BoolVal::True, "truthy boolean must be true");
    }

    #[test]
    fn falsy_narrowing_keeps_only_falsy_values() {
        // {null, [0,5], "a"|"" , Top-bool, ref} — falsy branch.
        let v = StateValue::null()
            .join(&StateValue::number(Interval { lo: 0.0, hi: 5.0 }))
            .join(&StateValue::str_set(
                ["a".to_string(), String::new()].into_iter().collect(),
            ))
            .join(&StateValue::boolean(BoolVal::Top))
            .join(&StateValue::reference(Stability::Unstable));
        let f = v.narrow_falsy();
        assert!(f.null);
        assert_eq!(f.num, Interval::point(0.0), "only 0 is a falsy number");
        assert_eq!(
            f.str,
            StrConst::singleton(String::new()),
            "only \"\" is a falsy string"
        );
        assert_eq!(f.boolean, BoolVal::False);
        assert_eq!(f.reference, Stability::Bottom, "references are never falsy");
        // A truthy-only value narrows to ⊥ on the falsy branch.
        assert!(
            StateValue::reference(Stability::Unstable)
                .join(&num_point(3.0))
                .narrow_falsy()
                .is_bottom()
        );
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
        let j = str_singleton("x").join(&StateValue::str_top());
        assert_eq!(j, StateValue::str_top());
    }

    #[test]
    fn str_join_beyond_threshold_widens_to_str() {
        let mut v = str_singleton("a");
        for c in ["b", "c", "d", "e"] {
            v = v.join(&str_singleton(c));
        }
        assert_eq!(v, StateValue::str_top());
    }

    #[test]
    fn str_partial_ord_subset() {
        let single = str_singleton("a");
        let pair = str_pair("a", "b");
        assert!(single < pair);
        assert!(!(pair < single));
        assert!(single <= StateValue::str_top());
    }

    #[test]
    fn str_meet_gives_intersection() {
        let a = str_pair("x", "y");
        let b = str_pair("y", "z");
        let m = a.meet(&b);
        assert_eq!(m, str_singleton("y"));
    }

    // ── Pointwise order ───────────────────────────────────────────────────────

    #[test]
    fn mixed_direction_slots_are_incomparable() {
        // a has a wider num slot, b has a wider str slot → incomparable.
        let a = StateValue::number(Interval { lo: 0.0, hi: 5.0 });
        let b = num_point(1.0).join(&str_singleton("s"));
        assert!(a.partial_cmp(&b).is_none());
    }

    #[test]
    fn join_is_upper_bound() {
        let a = StateValue::null().join(&num_point(1.0));
        let b = str_singleton("x");
        let j = a.join(&b);
        assert!(a <= j);
        assert!(b <= j);
    }

    // ── ComponentSetter (setter slot) ─────────────────────────────────────────

    fn cs(comp: &str, label: usize) -> StateValue {
        StateValue::component_setter(comp.to_string(), label)
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
    fn component_setter_join_different_loses_identity_stays_stable() {
        // Old lattice collapsed to Reference(Stable); the product keeps the
        // setter slot at ⊤ — still Stable, identity lost.
        for j in [
            cs("Foo", 0).join(&cs("Bar", 0)),
            cs("Foo", 0).join(&cs("Foo", 1)),
        ] {
            assert_eq!(j.setter, SetterVal::Top);
            assert!(j.as_setter().is_none());
            assert_eq!(j.to_stability(), Stability::Stable);
        }
    }

    #[test]
    fn component_setter_join_with_ref_keeps_both_slots() {
        let j = cs("Foo", 0).join(&StateValue::reference(Stability::Stable));
        assert_eq!(j.setter, SetterVal::One("Foo".to_string(), 0));
        assert_eq!(j.reference, Stability::Stable);
        // Mixed with a reference → no longer an exact setter.
        assert!(j.as_setter().is_none());
        assert_eq!(j.to_stability(), Stability::Stable);
    }

    #[test]
    fn component_setter_join_with_unstable_ref_is_unstable() {
        // Motion-wins: the unstable reference slot dominates the stable setter.
        let j = cs("Foo", 0).join(&StateValue::reference(Stability::Unstable));
        assert_eq!(j.to_stability(), Stability::Unstable);
    }

    #[test]
    fn nullable_widened_number_is_unstable() {
        // {null ∪ number[0,+∞)} — a widened nullable counter changes every
        // render; motion-wins must report Unstable (this is what lets
        // `all_deps_unstable` see through `useState(null)` counters).
        let v = StateValue::null().join(&StateValue::number(Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        }));
        assert_eq!(v.to_stability(), Stability::Unstable);
        // Top stays Unknown: opaque values never claim definite instability.
        assert_eq!(StateValue::top().to_stability(), Stability::Unknown);
    }

    #[test]
    fn component_setter_join_with_top_gives_top() {
        assert!(cs("Foo", 0).join(&StateValue::top()).is_top_value());
    }

    #[test]
    fn component_setter_join_with_bottom_gives_self() {
        assert_eq!(cs("Foo", 0).join(&StateValue::bottom()), cs("Foo", 0));
    }

    #[test]
    fn component_setter_as_setter_extracts_payload() {
        let v = cs("Foo", 3);
        assert_eq!(v.as_setter(), Some((&"Foo".to_string(), &3)));
        // Bottom / Top / mixed values expose no exact setter.
        assert!(StateValue::bottom().as_setter().is_none());
        assert!(StateValue::top().as_setter().is_none());
    }

    #[test]
    fn different_component_setters_incomparable() {
        assert!(cs("Foo", 0).partial_cmp(&cs("Bar", 0)).is_none());
    }

    #[test]
    fn component_setter_meet_same_is_identity() {
        assert_eq!(cs("Foo", 0).meet(&cs("Foo", 0)), cs("Foo", 0));
    }

    #[test]
    fn component_setter_meet_different_is_bottom() {
        assert!(cs("Foo", 0).meet(&cs("Bar", 0)).is_bottom());
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

    // ── is_unstable_reference_only ────────────────────────────────────────────

    #[test]
    fn unstable_reference_only_detection() {
        assert!(StateValue::reference(Stability::Unstable).is_unstable_reference_only());
        assert!(!StateValue::reference(Stability::Stable).is_unstable_reference_only());
        assert!(!StateValue::top().is_unstable_reference_only());
        assert!(
            !StateValue::reference(Stability::Unstable)
                .join(&StateValue::null())
                .is_unstable_reference_only()
        );
    }

    // ── Debug rendering ───────────────────────────────────────────────────────

    #[test]
    fn debug_renders_kind_union() {
        let v = StateValue::null().join(&num_point(1.0));
        assert_eq!(format!("{v:?}"), "number[1, 1]|null");
        assert_eq!(format!("{:?}", StateValue::bottom()), "⊥");
        assert_eq!(format!("{:?}", StateValue::top()), "⊤");
    }
}
