use std::cmp::Ordering;

use crate::domains::AbstractDomain;

/// Closed interval [lo, hi] over f64. `lo > hi` = bottom (empty).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
}

impl Interval {
    pub fn point(v: f64) -> Self {
        Interval { lo: v, hi: v }
    }

    pub fn top() -> Self {
        Interval {
            lo: f64::NEG_INFINITY,
            hi: f64::INFINITY,
        }
    }

    /// Empty interval represents ⊥ for the numeric sub-lattice.
    pub fn bottom() -> Self {
        Interval {
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
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

    /// Least upper bound: smallest interval containing both.
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
        Interval { lo, hi }
    }

    pub fn add(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: self.lo + other.lo,
            hi: self.hi + other.hi,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: self.lo - other.hi,
            hi: self.hi - other.lo,
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
        Interval { lo, hi }
    }

    pub fn neg(&self) -> Self {
        if self.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: -self.hi,
            hi: -self.lo,
        }
    }

    // Narrowing: restrict interval to satisfy a comparison against a literal `v`.
    pub fn narrow_lt(&self, v: f64) -> Self {
        Interval {
            lo: self.lo,
            hi: self.hi.min(v - 1.0),
        }
    }
    pub fn narrow_leq(&self, v: f64) -> Self {
        Interval {
            lo: self.lo,
            hi: self.hi.min(v),
        }
    }
    pub fn narrow_gt(&self, v: f64) -> Self {
        Interval {
            lo: self.lo.max(v + 1.0),
            hi: self.hi,
        }
    }
    pub fn narrow_geq(&self, v: f64) -> Self {
        Interval {
            lo: self.lo.max(v),
            hi: self.hi,
        }
    }
    pub fn narrow_eq(&self, v: f64) -> Self {
        if self.lo <= v && v <= self.hi {
            Interval::point(v)
        } else {
            Interval::bottom()
        }
    }
    /// Conservative: can't split an interval at a point; return self.
    pub fn narrow_neq(&self, _v: f64) -> Self {
        *self
    }
}

/// [a,b] ≤ [c,d] iff [a,b] ⊆ [c,d] (i.e. c ≤ a && b ≤ d).
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
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        Interval { lo, hi } // bottom if lo > hi
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
        let a = Interval { lo: 0.0, hi: 5.0 };
        let b = Interval { lo: -1.0, hi: 5.0 };
        let w = a.widen(&b);
        assert!(w.lo.is_infinite() && w.lo < 0.0);
        assert_eq!(w.hi, 5.0);
    }

    #[test]
    fn interval_add() {
        let a = Interval { lo: 0.0, hi: 3.0 };
        let b = Interval::point(1.0);
        let r = a.add(&b);
        assert_eq!(r.lo, 1.0);
        assert_eq!(r.hi, 4.0);
    }

    #[test]
    fn widen_to_jumps_to_threshold_not_infinity() {
        // [0,5] grows to [0,6]; threshold 10 encloses 6 → hi = 10, not +∞.
        let a = Interval { lo: 0.0, hi: 5.0 };
        let b = Interval { lo: 0.0, hi: 6.0 };
        let w = a.widen_to(&b, &[10.0]);
        assert_eq!(w.lo, 0.0);
        assert_eq!(w.hi, 10.0);
    }

    #[test]
    fn widen_to_goes_infinity_when_no_threshold_encloses() {
        // grows past every threshold → +∞ (still sound).
        let a = Interval { lo: 0.0, hi: 5.0 };
        let b = Interval { lo: 0.0, hi: 11.0 };
        let w = a.widen_to(&b, &[10.0]);
        assert!(w.hi.is_infinite() && w.hi > 0.0);
    }

    #[test]
    fn widen_to_picks_tightest_enclosing_threshold() {
        let a = Interval { lo: 0.0, hi: 5.0 };
        let b = Interval { lo: 0.0, hi: 12.0 };
        let w = a.widen_to(&b, &[10.0, 20.0, 100.0]);
        assert_eq!(w.hi, 20.0); // smallest threshold ≥ 12
    }

    #[test]
    fn widen_to_lower_bound_threshold() {
        // lower bound shrinks; threshold -5 is the largest ≤ -3.
        let a = Interval { lo: 0.0, hi: 5.0 };
        let b = Interval { lo: -3.0, hi: 5.0 };
        let w = a.widen_to(&b, &[-5.0, 0.0]);
        assert_eq!(w.lo, -5.0);
        assert_eq!(w.hi, 5.0);
    }

    #[test]
    fn widen_to_empty_thresholds_equals_plain_widen() {
        let a = Interval { lo: 0.0, hi: 5.0 };
        let b = Interval { lo: 0.0, hi: 6.0 };
        assert_eq!(a.widen_to(&b, &[]), a.widen(&b));
    }

    #[test]
    fn widen_to_stable_bound_untouched() {
        // hi does not grow → keep it; only lo handling differs.
        let a = Interval { lo: 0.0, hi: 5.0 };
        let b = Interval { lo: 0.0, hi: 3.0 };
        let w = a.widen_to(&b, &[10.0]);
        assert_eq!(w, Interval { lo: 0.0, hi: 5.0 });
    }

    #[test]
    fn widen_to_is_sound_superset_of_hull() {
        // Result must contain the hull (over-approximation preserved).
        let a = Interval { lo: 2.0, hi: 5.0 };
        let b = Interval { lo: 0.0, hi: 8.0 };
        let w = a.widen_to(&b, &[10.0]);
        let h = a.hull(&b);
        assert!(w.lo <= h.lo && w.hi >= h.hi, "widen_to must be ⊒ hull");
    }

    #[test]
    fn interval_partial_ord() {
        let narrow = Interval { lo: 1.0, hi: 2.0 };
        let wide = Interval { lo: 0.0, hi: 5.0 };
        assert!(narrow < wide);
        assert!(!(wide < narrow));
    }
}
