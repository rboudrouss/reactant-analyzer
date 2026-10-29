use std::cmp::Ordering;

use crate::domains::AbstractDomain;

/// Closed interval [lo, hi] over f64. `lo > hi` = bottom (empty).
///
/// `is_int` records whether every concrete value the interval denotes is an
/// integer (`true` = proven all-integer; `false` = may contain non-integers).
/// It is a *precision* annotation only: it lets `narrow_lt`/`narrow_gt` apply
/// the integer tightening `x < 5 ⟹ x ≤ 4` (ADR-014 threshold widening relies on
/// this to bound counting loops) while staying sound over reals when the flag is
/// clear — a float state `x = 1.7` under `x < 2` must keep 1.7, not drop it.
/// Because it carries no set-membership information beyond `[lo, hi]`, it is
/// deliberately excluded from `PartialEq`/`PartialOrd`: two intervals with the
/// same bounds are equal (and the fixpoint converges) regardless of `is_int`.
///
/// `may_neg_zero` is a membership fact, so it does take part: a `0` in
/// `[lo, hi]` may be `-0`. The bounds compare `-0 == 0`, as `===` does, but
/// `Object.is(0, -0)` is false, and React compares state with `Object.is`: a
/// slot alternating `0` and `-0` re-renders every time (#208). Literals are
/// never `-0` (`-0` is the negation of `0`), so the bit is set only by an
/// operation that can produce it (`-x`, `*`, `%`, `**`, `-0 + -0`, a parse
/// of `"-0"`) and by ⊤; joins OR it. It is meaningful only while `0` is in
/// the bounds ([`Self::neg_zero`]).
#[derive(Debug, Clone, Copy)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
    pub is_int: bool,
    pub may_neg_zero: bool,
}

impl PartialEq for Interval {
    fn eq(&self, other: &Self) -> bool {
        self.lo == other.lo && self.hi == other.hi && self.neg_zero() == other.neg_zero()
    }
}

impl Interval {
    /// The interval holding `v` alone: a `-0` constant is the point `0` that
    /// may be `-0`.
    pub fn point(v: f64) -> Self {
        Interval {
            lo: v,
            hi: v,
            is_int: v.is_finite() && v.fract() == 0.0,
            may_neg_zero: v == 0.0 && v.is_sign_negative(),
        }
    }

    pub fn top() -> Self {
        Interval {
            lo: f64::NEG_INFINITY,
            hi: f64::INFINITY,
            is_int: false,
            may_neg_zero: true,
        }
    }

    /// Empty interval represents ⊥ for the numeric sub-lattice. `is_int` is
    /// vacuously true (the empty set contains no non-integer) and never read:
    /// every combinator short-circuits on `is_bottom` before touching it.
    pub fn bottom() -> Self {
        Interval {
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
            is_int: true,
            may_neg_zero: false,
        }
    }

    pub fn is_bottom(&self) -> bool {
        self.lo > self.hi
    }

    pub fn is_point(&self) -> bool {
        !self.is_bottom() && self.lo == self.hi
    }

    /// May the interval hold `-0`?
    pub fn neg_zero(&self) -> bool {
        self.may_neg_zero && self.holds(0.0)
    }

    /// One concrete value under `Object.is`: a point, unless it is a `0` that
    /// may be `-0` (#208).
    pub fn is_one_value(&self) -> bool {
        self.is_point() && !self.neg_zero()
    }

    /// `self` with the `-0` bit set to `may`.
    fn with_neg_zero(self, may: bool) -> Self {
        Interval {
            may_neg_zero: may,
            ..self
        }
    }

    pub fn is_top(&self) -> bool {
        self.lo == f64::NEG_INFINITY && self.hi == f64::INFINITY
    }

    /// Least upper bound: smallest interval containing both. Integer-valued only
    /// if both operands are (the union of two integer sets is integer-valued).
    pub fn hull(&self, other: &Self) -> Self {
        if self.is_bottom() {
            return *other;
        }
        if other.is_bottom() {
            return *self;
        }
        Interval {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
            is_int: self.is_int && other.is_int,
            may_neg_zero: self.neg_zero() || other.neg_zero(),
        }
    }

    /// Widening: if other grows the bound beyond self, jump to ±∞.
    pub fn widen(&self, other: &Self) -> Self {
        if self.is_bottom() {
            return *other;
        }
        if other.is_bottom() {
            return *self;
        }
        Interval {
            lo: if other.lo < self.lo {
                f64::NEG_INFINITY
            } else {
                self.lo
            },
            hi: if other.hi > self.hi {
                f64::INFINITY
            } else {
                self.hi
            },
            is_int: self.is_int && other.is_int,
            may_neg_zero: self.neg_zero() || other.neg_zero(),
        }
    }

    /// Threshold widening ("widening up to"). A bound that grows jumps to the
    /// tightest enclosing threshold instead of ±∞; ±∞ is used only when no finite
    /// threshold encloses the grown bound. `thresholds` need not be sorted.
    ///
    /// Sound: result ⊒ self.hull(other) (bounds only ever loosen), and the set is
    /// finite so the ascending chain still stabilises.
    pub fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        if self.is_bottom() {
            return *other;
        }
        if other.is_bottom() {
            return *self;
        }
        let lo = if other.lo < self.lo {
            // Largest threshold ≤ other.lo, else -∞.
            thresholds
                .iter()
                .copied()
                .filter(|&t| t <= other.lo)
                .fold(f64::NEG_INFINITY, f64::max)
        } else {
            self.lo
        };
        let hi = if other.hi > self.hi {
            // Smallest threshold ≥ other.hi, else +∞.
            thresholds
                .iter()
                .copied()
                .filter(|&t| t >= other.hi)
                .fold(f64::INFINITY, f64::min)
        } else {
            self.hi
        };
        Interval {
            lo,
            hi,
            is_int: self.is_int && other.is_int,
            may_neg_zero: self.neg_zero() || other.neg_zero(),
        }
    }

    /// Does the interval hold `v`? A closed bound is a member: an infinite one
    /// is the value `±Infinity`, which JS arithmetic reaches.
    fn holds(&self, v: f64) -> bool {
        self.lo <= v && v <= self.hi
    }

    /// Is one of the bounds infinite?
    fn unbounded(&self) -> bool {
        self.lo.is_infinite() || self.hi.is_infinite()
    }

    /// `self + other`. `None` when the sum may be `NaN` — `+∞` on one side and
    /// `−∞` on the other — which no interval holds (#217): a `NaN` bound
    /// would read as ⊥ and kill every value computed from it.
    pub fn add(&self, other: &Self) -> Option<Self> {
        if self.is_bottom() || other.is_bottom() {
            return Some(Interval::bottom());
        }
        if (self.holds(f64::INFINITY) && other.holds(f64::NEG_INFINITY))
            || (self.holds(f64::NEG_INFINITY) && other.holds(f64::INFINITY))
        {
            return None;
        }
        // `-0 + -0` is the only sum that is `-0`.
        Some(Interval {
            lo: self.lo + other.lo,
            hi: self.hi + other.hi,
            is_int: self.is_int && other.is_int,
            may_neg_zero: self.neg_zero() && other.neg_zero(),
        })
    }

    /// `self - other`, `None` when it may be `∞ − ∞`, as in [`Self::add`].
    pub fn sub(&self, other: &Self) -> Option<Self> {
        self.add(&other.neg())
    }

    /// `self * other`: the extremes sit at the corners. `None` when the
    /// product may be `0 × ∞`, which is `NaN` (#217): the corner folds skip a
    /// `NaN`, and `[0,0] × [−∞,+∞]` used to come out ⊥.
    pub fn mul(&self, other: &Self) -> Option<Self> {
        if self.is_bottom() || other.is_bottom() {
            return Some(Interval::bottom());
        }
        if (self.holds(0.0) && other.unbounded()) || (other.holds(0.0) && self.unbounded()) {
            return None;
        }
        let products = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        let lo = products.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = products.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        // A zero product is `-0` when a factor may be negative or `-0`
        // (`0 * -1`, an underflow of `-1e-200 * 1e-200`).
        let signed = |i: &Self| i.lo < 0.0 || i.neg_zero();
        Some(Interval {
            lo,
            hi,
            is_int: self.is_int && other.is_int,
            may_neg_zero: signed(self) || signed(other),
        })
    }

    /// `-self`: `-0` wherever `self` may hold `+0`.
    pub fn neg(&self) -> Self {
        if self.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: -self.hi,
            hi: -self.lo,
            is_int: self.is_int,
            may_neg_zero: self.holds(0.0),
        }
    }

    /// `self % other` with JS semantics: the result takes the dividend's sign
    /// and is strictly smaller in magnitude than both the divisor and the
    /// dividend. `None` when the divisor may be zero — the result may then be
    /// `NaN`, which no interval holds.
    pub fn rem(&self, other: &Self) -> Option<Self> {
        if self.is_bottom() || other.is_bottom() {
            return Some(Interval::bottom());
        }
        if other.lo <= 0.0 && other.hi >= 0.0 {
            return None;
        }
        let is_int = self.is_int && other.is_int;
        let magnitude = other.lo.abs().max(other.hi.abs());
        // Strictly below the divisor's magnitude: one less on integers, the
        // closed bound otherwise.
        let bound = if is_int { magnitude - 1.0 } else { magnitude };
        Some(Interval {
            lo: if self.lo >= 0.0 {
                0.0
            } else {
                self.lo.max(-bound)
            },
            hi: if self.hi <= 0.0 {
                0.0
            } else {
                self.hi.min(bound)
            },
            is_int,
            // A zero remainder takes the dividend's sign: `-4 % 2` is `-0`.
            may_neg_zero: self.lo < 0.0 || self.neg_zero(),
        })
    }

    /// `self ** other`. On the non-negative quadrant the function is monotone
    /// in each argument, so the extremes sit at the corners of the operand
    /// box. `None` outside it: a negative base with a fractional exponent is
    /// `NaN`, and a negative exponent of zero is infinite.
    pub fn pow(&self, other: &Self) -> Option<Self> {
        if self.is_bottom() || other.is_bottom() {
            return Some(Interval::bottom());
        }
        if self.lo < 0.0 || other.lo < 0.0 {
            return None;
        }
        let powers = [
            self.lo.powf(other.lo),
            self.lo.powf(other.hi),
            self.hi.powf(other.lo),
            self.hi.powf(other.hi),
        ];
        Some(Interval {
            lo: powers.iter().cloned().fold(f64::INFINITY, f64::min),
            hi: powers.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            is_int: self.is_int && other.is_int,
            // `(-0) ** 1` is `-0`; a non-negative base gives `+0`.
            may_neg_zero: self.neg_zero(),
        })
    }

    // Narrowing: restrict interval to satisfy a comparison against a literal `v`.
    //
    // For `<`/`>` the sound bound over reals is `v` itself (the excluded
    // endpoint is kept — a float state `x = 1.7` under `x < 2` would lose 1.7 if
    // we used `v ∓ 1`, an unsound value FN). Integer tightening (`x < 5 ⟹ x ≤ 4`)
    // is applied only when `is_int` proves every value is an integer; that is
    // what keeps ADR-014 threshold widening able to bound `i < N; i++` loops.
    //
    // A `-0` compares as `0` under every one of these, so the bit survives
    // with the `0` it rides on.
    pub fn narrow_lt(&self, v: f64) -> Self {
        let bound = if self.is_int { v.ceil() - 1.0 } else { v };
        Interval {
            lo: self.lo,
            hi: self.hi.min(bound),
            ..*self
        }
    }
    pub fn narrow_leq(&self, v: f64) -> Self {
        let bound = if self.is_int { v.floor() } else { v };
        Interval {
            lo: self.lo,
            hi: self.hi.min(bound),
            ..*self
        }
    }
    pub fn narrow_gt(&self, v: f64) -> Self {
        let bound = if self.is_int { v.floor() + 1.0 } else { v };
        Interval {
            lo: self.lo.max(bound),
            hi: self.hi,
            ..*self
        }
    }
    pub fn narrow_geq(&self, v: f64) -> Self {
        let bound = if self.is_int { v.ceil() } else { v };
        Interval {
            lo: self.lo.max(bound),
            hi: self.hi,
            ..*self
        }
    }
    pub fn narrow_eq(&self, v: f64) -> Self {
        if self.lo <= v && v <= self.hi {
            let p = Interval::point(v);
            p.with_neg_zero(p.may_neg_zero || self.neg_zero())
        } else {
            Interval::bottom()
        }
    }
    /// Conservative: can't split an interval at an interior point; return self.
    /// Exception: a point interval equal to `v` is exactly excluded → ⊥.
    pub fn narrow_neq(&self, v: f64) -> Self {
        if self.is_point() && self.lo == v {
            Interval::bottom()
        } else {
            *self
        }
    }
}

/// `[a,b] ≤ [c,d]` iff `[a,b] ⊆ [c,d]` (i.e. c ≤ a && b ≤ d), a `-0` included.
/// `is_int` is a precision annotation and does not participate (see the
/// struct docs).
impl PartialOrd for Interval {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self == other {
            return Some(Ordering::Equal);
        }
        let within =
            |a: &Self, b: &Self| b.lo <= a.lo && a.hi <= b.hi && (!a.neg_zero() || b.neg_zero());
        let self_in_other = within(self, other);
        let other_in_self = within(other, self);
        match (self_in_other, other_in_self) {
            (true, false) => Some(Ordering::Less),
            (false, true) => Some(Ordering::Greater),
            (true, true) => Some(Ordering::Equal),
            (false, false) => None,
        }
    }
}

// ── AbstractDomain ────────────────────────────────────────────────────────────

impl AbstractDomain for Interval {
    fn bottom() -> Self {
        Interval::bottom()
    }
    fn top() -> Self {
        Interval::top()
    }
    fn is_bottom(&self) -> bool {
        Interval::is_bottom(self)
    }
    fn join(&self, other: &Self) -> Self {
        self.hull(other)
    }
    fn meet(&self, other: &Self) -> Self {
        // Intersection: integer-valued if *either* operand is (the meet is a
        // subset of each, so a subset of an all-integer set is all-integer).
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        Interval {
            lo,
            hi,
            is_int: self.is_int || other.is_int,
            may_neg_zero: self.neg_zero() && other.neg_zero(),
        } // bottom if lo > hi
    }
    fn widen(&self, other: &Self) -> Self {
        Interval::widen(self, other)
    }
    fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        Interval::widen_to(self, other, thresholds)
    }

    // Use fully-qualified inherent methods to avoid recursive trait dispatch.
    fn narrow_lt(self, v: f64) -> Self {
        Interval::narrow_lt(&self, v)
    }
    fn narrow_leq(self, v: f64) -> Self {
        Interval::narrow_leq(&self, v)
    }
    fn narrow_gt(self, v: f64) -> Self {
        Interval::narrow_gt(&self, v)
    }
    fn narrow_geq(self, v: f64) -> Self {
        Interval::narrow_geq(&self, v)
    }
    fn narrow_eq(self, v: f64) -> Self {
        Interval::narrow_eq(&self, v)
    }
    fn narrow_neq(self, v: f64) -> Self {
        Interval::narrow_neq(&self, v)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_point_is_stable() {
        let i = Interval::point(42.0);
        assert!(i.is_point());
        assert!(!i.is_bottom());
    }

    #[test]
    fn interval_hull() {
        let a = Interval::point(0.0);
        let b = Interval::point(1.0);
        let h = a.hull(&b);
        assert_eq!(h.lo, 0.0);
        assert_eq!(h.hi, 1.0);
    }

    fn ints(lo: f64, hi: f64) -> Interval {
        Interval {
            lo,
            hi,
            is_int: true,
            may_neg_zero: false,
        }
    }

    fn floats(lo: f64, hi: f64) -> Interval {
        Interval {
            lo,
            hi,
            is_int: false,
            may_neg_zero: false,
        }
    }

    #[test]
    fn rem_follows_the_dividend_sign_and_stays_below_the_divisor() {
        // Integers: strictly below the divisor means `k - 1`.
        assert_eq!(ints(0.0, 100.0).rem(&ints(7.0, 7.0)), Some(ints(0.0, 6.0)));
        // A negative dividend's zero remainder is `-0`.
        assert_eq!(
            ints(-100.0, -1.0).rem(&ints(7.0, 7.0)),
            Some(ints(-6.0, 0.0).with_neg_zero(true))
        );
        assert_eq!(
            ints(-100.0, 100.0).rem(&ints(-7.0, -7.0)),
            Some(ints(-6.0, 6.0).with_neg_zero(true))
        );
        // A dividend smaller than the divisor is its own bound.
        assert_eq!(ints(0.0, 2.0).rem(&ints(7.0, 7.0)), Some(ints(0.0, 2.0)));
        // Floats: the closed bound, and the result is not proven integer.
        let r = floats(0.0, 100.0).rem(&floats(2.5, 2.5)).unwrap();
        assert_eq!(r, floats(0.0, 2.5));
        assert!(!r.is_int);
        // A divisor interval that excludes zero bounds by its largest magnitude.
        assert_eq!(ints(0.0, 100.0).rem(&ints(2.0, 5.0)), Some(ints(0.0, 4.0)));
        // An unbounded divisor still fixes the sign.
        assert_eq!(
            ints(0.0, 100.0).rem(&ints(1.0, f64::INFINITY)),
            Some(ints(0.0, 100.0))
        );
    }

    #[test]
    fn rem_by_a_possible_zero_is_unrepresentable() {
        assert_eq!(ints(0.0, 9.0).rem(&ints(0.0, 3.0)), None);
        assert_eq!(ints(0.0, 9.0).rem(&ints(-3.0, 3.0)), None);
        assert_eq!(ints(0.0, 9.0).rem(&Interval::top()), None);
        assert_eq!(
            Interval::bottom().rem(&ints(3.0, 3.0)),
            Some(Interval::bottom())
        );
    }

    #[test]
    fn pow_takes_the_corners_of_the_non_negative_box() {
        assert_eq!(ints(2.0, 3.0).pow(&ints(2.0, 2.0)), Some(ints(4.0, 9.0)));
        // Below one the power decreases with the exponent: the corners still
        // hold both extremes.
        assert_eq!(
            floats(0.5, 2.0).pow(&ints(1.0, 2.0)),
            Some(floats(0.25, 4.0))
        );
        assert_eq!(ints(0.0, 0.0).pow(&ints(0.0, 3.0)), Some(ints(0.0, 1.0)));
        assert_eq!(
            ints(2.0, f64::INFINITY).pow(&ints(1.0, 1.0)),
            Some(ints(2.0, f64::INFINITY))
        );
        // Not proven integer once the exponent is not.
        assert!(!ints(4.0, 4.0).pow(&floats(0.5, 0.5)).unwrap().is_int);
    }

    #[test]
    fn pow_outside_the_non_negative_box_is_unrepresentable() {
        assert_eq!(ints(-2.0, 3.0).pow(&ints(2.0, 2.0)), None);
        assert_eq!(ints(0.0, 3.0).pow(&ints(-1.0, 2.0)), None);
        assert_eq!(
            ints(2.0, 2.0).pow(&Interval::bottom()),
            Some(Interval::bottom())
        );
    }

    #[test]
    fn interval_widen_grows_hi() {
        let a = Interval::point(0.0);
        let b = Interval::point(1.0);
        let w = a.widen(&b);
        assert_eq!(w.lo, 0.0);
        assert!(w.hi.is_infinite());
    }

    #[test]
    fn interval_widen_shrinks_lo() {
        let a = Interval::point(0.0).hull(&Interval::point(5.0));
        let b = Interval::point(-1.0).hull(&Interval::point(5.0));
        let w = a.widen(&b);
        assert!(w.lo.is_infinite() && w.lo < 0.0);
        assert_eq!(w.hi, 5.0);
    }

    #[test]
    fn interval_add() {
        let a = Interval::point(0.0).hull(&Interval::point(3.0));
        let b = Interval::point(1.0);
        let r = a.add(&b).unwrap();
        assert_eq!(r.lo, 1.0);
        assert_eq!(r.hi, 4.0);
    }

    /// `0 × ∞` is `NaN`: the corner folds used to skip it and return ⊥ for
    /// `[0,0] × [−∞,+∞]`, killing every value computed from it (#217).
    #[test]
    fn mul_by_a_possible_zero_times_infinity_is_unrepresentable() {
        let zero = ints(0.0, 0.0);
        assert_eq!(zero.mul(&Interval::top()), None);
        assert_eq!(Interval::top().mul(&zero), None);
        assert_eq!(
            ints(-1.0, 1.0).mul(&ints(1.0, f64::INFINITY)),
            None,
            "0 is inside the left operand, +∞ a bound of the right one"
        );
        assert_eq!(
            ints(f64::INFINITY, f64::INFINITY).mul(&ints(0.0, 0.0)),
            None
        );
        // No zero against an unbounded side, or nothing unbounded: exact.
        assert_eq!(
            ints(2.0, 2.0).mul(&ints(1.0, f64::INFINITY)),
            Some(ints(2.0, f64::INFINITY))
        );
        assert_eq!(
            ints(0.0, 3.0).mul(&ints(-2.0, 2.0)),
            Some(ints(-6.0, 6.0).with_neg_zero(true))
        );
        assert_eq!(
            Interval::bottom().mul(&Interval::top()),
            Some(Interval::bottom())
        );
    }

    /// `∞ − ∞` is `NaN`, whichever of `+` and `-` spells it (#217).
    #[test]
    fn opposite_infinities_add_to_an_unrepresentable_value() {
        let pos = ints(f64::INFINITY, f64::INFINITY);
        let neg = ints(f64::NEG_INFINITY, f64::NEG_INFINITY);
        assert_eq!(pos.add(&neg), None);
        assert_eq!(pos.sub(&pos), None);
        assert_eq!(Interval::top().add(&Interval::top()), None);
        assert_eq!(
            ints(0.0, f64::INFINITY).sub(&ints(1.0, f64::INFINITY)),
            None
        );
        // One side bounded: the widened counter `x + 1` and `x - 1` stay exact.
        assert_eq!(
            ints(0.0, f64::INFINITY).add(&ints(1.0, 1.0)),
            Some(ints(1.0, f64::INFINITY))
        );
        assert!(Interval::top().sub(&ints(1.0, 1.0)).unwrap().is_top());
        assert_eq!(
            ints(0.0, f64::INFINITY).sub(&ints(f64::NEG_INFINITY, 0.0)),
            Some(ints(0.0, f64::INFINITY))
        );
    }

    #[test]
    fn widen_to_jumps_to_threshold_not_infinity() {
        // [0,5] grows to [0,6]; threshold 10 encloses 6 → hi = 10, not +∞.
        let a = Interval::point(0.0).hull(&Interval::point(5.0));
        let b = Interval::point(0.0).hull(&Interval::point(6.0));
        let w = a.widen_to(&b, &[10.0]);
        assert_eq!(w.lo, 0.0);
        assert_eq!(w.hi, 10.0);
    }

    #[test]
    fn widen_to_goes_infinity_when_no_threshold_encloses() {
        // grows past every threshold → +∞ (still sound).
        let a = Interval::point(0.0).hull(&Interval::point(5.0));
        let b = Interval::point(0.0).hull(&Interval::point(11.0));
        let w = a.widen_to(&b, &[10.0]);
        assert!(w.hi.is_infinite() && w.hi > 0.0);
    }

    #[test]
    fn widen_to_picks_tightest_enclosing_threshold() {
        let a = Interval::point(0.0).hull(&Interval::point(5.0));
        let b = Interval::point(0.0).hull(&Interval::point(12.0));
        let w = a.widen_to(&b, &[10.0, 20.0, 100.0]);
        assert_eq!(w.hi, 20.0); // smallest threshold ≥ 12
    }

    #[test]
    fn widen_to_lower_bound_threshold() {
        // lower bound shrinks; threshold -5 is the largest ≤ -3.
        let a = Interval::point(0.0).hull(&Interval::point(5.0));
        let b = Interval::point(-3.0).hull(&Interval::point(5.0));
        let w = a.widen_to(&b, &[-5.0, 0.0]);
        assert_eq!(w.lo, -5.0);
        assert_eq!(w.hi, 5.0);
    }

    #[test]
    fn widen_to_empty_thresholds_equals_plain_widen() {
        let a = Interval::point(0.0).hull(&Interval::point(5.0));
        let b = Interval::point(0.0).hull(&Interval::point(6.0));
        assert_eq!(a.widen_to(&b, &[]), a.widen(&b));
    }

    #[test]
    fn widen_to_stable_bound_untouched() {
        // hi does not grow → keep it; only lo handling differs.
        let a = Interval::point(0.0).hull(&Interval::point(5.0));
        let b = Interval::point(0.0).hull(&Interval::point(3.0));
        let w = a.widen_to(&b, &[10.0]);
        assert_eq!(w, Interval::point(0.0).hull(&Interval::point(5.0)));
    }

    #[test]
    fn widen_to_is_sound_superset_of_hull() {
        // Result must contain the hull (over-approximation preserved).
        let a = Interval::point(2.0).hull(&Interval::point(5.0));
        let b = Interval::point(0.0).hull(&Interval::point(8.0));
        let w = a.widen_to(&b, &[10.0]);
        let h = a.hull(&b);
        assert!(w.lo <= h.lo && w.hi >= h.hi, "widen_to must be ⊒ hull");
    }

    #[test]
    fn interval_partial_ord() {
        let narrow = Interval::point(1.0).hull(&Interval::point(2.0));
        let wide = Interval::point(0.0).hull(&Interval::point(5.0));
        assert_eq!(narrow.partial_cmp(&wide), Some(Ordering::Less));
    }

    // ── Integrality (ADR-014 precision vs. float soundness) ─────────────────

    #[test]
    fn narrow_lt_float_keeps_boundary_value() {
        // The FN this fixes: `x = 1.7` under `x < 2` must keep 1.7, not drop it
        // to ⊥ via an integer step. A non-integer point interval is not `is_int`.
        let x = Interval::point(1.7);
        assert!(!x.is_int);
        let n = x.narrow_lt(2.0);
        assert!(!n.is_bottom(), "1.7 must survive `< 2`");
        assert_eq!(n, Interval::point(1.7));
    }

    #[test]
    fn narrow_lt_integer_still_tightens() {
        // A proven-integer interval keeps the ADR-014 tightening `x < 5 ⟹ x ≤ 4`.
        let x = Interval::point(0.0).hull(&Interval::point(9.0));
        assert!(x.is_int);
        assert_eq!(x.narrow_lt(5.0).hi, 4.0);
        assert_eq!(x.narrow_gt(5.0).lo, 6.0);
    }

    #[test]
    fn integrality_lost_through_float_arithmetic() {
        // int ⊕ non-int ⟹ result is no longer proven-integer.
        let i = Interval::point(3.0);
        let f = Interval::point(0.5);
        assert!(!i.add(&f).unwrap().is_int);
    }

    // ── `-0` (#208) ─────────────────────────────────────────────────────────

    /// `Object.is(0, -0)` is false: a literal `0` is one value, a negated one
    /// may be another, and the two are different abstract values.
    #[test]
    fn a_literal_zero_is_one_value_a_negated_zero_is_not() {
        let zero = Interval::point(0.0);
        assert!(zero.is_one_value());
        let neg = zero.neg();
        assert!(neg.neg_zero());
        assert!(!neg.is_one_value());
        assert_ne!(zero, neg);
        assert!(zero < zero.hull(&neg));
        assert!(Interval::point(-0.0).neg_zero());
        assert!(Interval::point(5.0).neg().is_one_value());
        assert!(!Interval::top().is_one_value() && Interval::top().neg_zero());
    }

    /// Only an operation that can produce `-0` sets the bit, and it survives
    /// a join, a widening and a comparison with `0`.
    #[test]
    fn only_a_sign_carrying_operation_produces_neg_zero() {
        let zero = ints(0.0, 0.0);
        let neg = zero.neg();
        // `0 * -1`, `-4 % 2`, `-0 + -0`, `-0 - 0` are `-0`.
        assert!(zero.mul(&ints(-1.0, -1.0)).unwrap().neg_zero());
        assert!(ints(-4.0, -4.0).rem(&ints(2.0, 2.0)).unwrap().neg_zero());
        assert!(neg.add(&neg).unwrap().neg_zero());
        assert!(neg.sub(&zero).unwrap().neg_zero());
        // `0 + 0`, `0 - 0`, `-0 + 0`, `0 * 3`, `0 % 2`, `0 ** 1` are `+0`.
        assert!(zero.add(&zero).unwrap().is_one_value());
        assert!(zero.sub(&zero).unwrap().is_one_value());
        assert!(neg.add(&zero).unwrap().is_one_value());
        assert!(zero.mul(&ints(3.0, 3.0)).unwrap().is_one_value());
        assert!(zero.rem(&ints(2.0, 2.0)).unwrap().is_one_value());
        assert!(zero.pow(&ints(1.0, 1.0)).unwrap().is_one_value());
        // Joins, widenings and `x === 0` keep it.
        assert!(zero.hull(&neg).neg_zero());
        assert!(zero.widen(&neg).neg_zero());
        assert!(neg.hull(&ints(1.0, 5.0)).narrow_eq(0.0).neg_zero());
        assert!(neg.hull(&ints(1.0, 5.0)).narrow_lt(1.0).neg_zero());
        assert!(neg.narrow_neq(0.0).is_bottom());
    }
}
