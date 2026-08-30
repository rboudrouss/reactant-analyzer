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
#[derive(Debug, Clone, Copy)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
    pub is_int: bool,
}

impl PartialEq for Interval {
    fn eq(&self, other: &Self) -> bool {
        self.lo == other.lo && self.hi == other.hi
    }
}

impl Interval {
    pub fn point(v: f64) -> Self {
        Interval {
            lo: v,
            hi: v,
            is_int: v.is_finite() && v.fract() == 0.0,
        }
    }

    pub fn top() -> Self {
        Interval {
            lo: f64::NEG_INFINITY,
            hi: f64::INFINITY,
            is_int: false,
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
        }
    }

    pub fn is_bottom(&self) -> bool {
        self.lo > self.hi
    }

    pub fn is_point(&self) -> bool {
        !self.is_bottom() && self.lo == self.hi
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
        }
    }

    pub fn add(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: self.lo + other.lo,
            hi: self.hi + other.hi,
            is_int: self.is_int && other.is_int,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: self.lo - other.hi,
            hi: self.hi - other.lo,
            is_int: self.is_int && other.is_int,
        }
    }

    pub fn mul(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        let products = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        let lo = products.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = products.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Interval {
            lo,
            hi,
            is_int: self.is_int && other.is_int,
        }
    }

    pub fn neg(&self) -> Self {
        if self.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: -self.hi,
            hi: -self.lo,
            is_int: self.is_int,
        }
    }

    // Narrowing: restrict interval to satisfy a comparison against a literal `v`.
    //
    // For `<`/`>` the sound bound over reals is `v` itself (the excluded
    // endpoint is kept — a float state `x = 1.7` under `x < 2` would lose 1.7 if
    // we used `v ∓ 1`, an unsound value FN). Integer tightening (`x < 5 ⟹ x ≤ 4`)
    // is applied only when `is_int` proves every value is an integer; that is
    // what keeps ADR-014 threshold widening able to bound `i < N; i++` loops.
    pub fn narrow_lt(&self, v: f64) -> Self {
        let bound = if self.is_int { v.ceil() - 1.0 } else { v };
        Interval {
            lo: self.lo,
            hi: self.hi.min(bound),
            is_int: self.is_int,
        }
    }
    pub fn narrow_leq(&self, v: f64) -> Self {
        let bound = if self.is_int { v.floor() } else { v };
        Interval {
            lo: self.lo,
            hi: self.hi.min(bound),
            is_int: self.is_int,
        }
    }
    pub fn narrow_gt(&self, v: f64) -> Self {
        let bound = if self.is_int { v.floor() + 1.0 } else { v };
        Interval {
            lo: self.lo.max(bound),
            hi: self.hi,
            is_int: self.is_int,
        }
    }
    pub fn narrow_geq(&self, v: f64) -> Self {
        let bound = if self.is_int { v.ceil() } else { v };
        Interval {
            lo: self.lo.max(bound),
            hi: self.hi,
            is_int: self.is_int,
        }
    }
    pub fn narrow_eq(&self, v: f64) -> Self {
        if self.lo <= v && v <= self.hi {
            Interval::point(v)
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

/// `[a,b] ≤ [c,d]` iff `[a,b] ⊆ [c,d]` (i.e. c ≤ a && b ≤ d). Bounds only — `is_int`
/// is a precision annotation and does not participate (see the struct docs).
impl PartialOrd for Interval {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self == other {
            return Some(Ordering::Equal);
        }
        let self_in_other = other.lo <= self.lo && self.hi <= other.hi;
        let other_in_self = self.lo <= other.lo && other.hi <= self.hi;
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
        let r = a.add(&b);
        assert_eq!(r.lo, 1.0);
        assert_eq!(r.hi, 4.0);
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
        assert!(!i.add(&f).is_int);
    }
}
