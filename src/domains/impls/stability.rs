use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::{
    domains::AbstractDomain,
    ir::{ComponentId, QualifiedSlot, types::HookLabel},
};

/// Threshold on `Versioned` label sets before widening to `VersionedTop`
/// (same pattern as `StrConst`).
pub const VERSIONED_LABELS_THRESHOLD: usize = 4;

/// Stability lattice: bounds on the *change trace* of a value — the set of
/// renders where `Object.is(vᵢ, vᵢ₋₁)` fails (the only thing React observes).
///
/// Two kinds of bounds coexist (ADR-017):
/// - **may** bound (over-approx): used by rules to *stay silent* soundly.
/// - **must** bound (under-approx): used by rules to *fire* without FPs.
///
/// ```text
///                 MayBeFresh  (⊤)
///                /           \
///            Unknown          |
///               |             |
///         VersionedTop     PerRender
///               |             |
///    Versioned(S) ⊆-chains    |
///               |             |
///            Stable           |
///                \           /
///                 Bottom  (⊥)
/// ```
///
/// `PerRender` is incomparable with every other point but the ends — not an
/// opposite, a different bound. What separates `Unknown` from `MayBeFresh`
/// is not the change trace (neither bounds it) but its cause (#304): the
/// re-run class reads an `Unknown` value through what it is computed from
/// (`render_deps`), and an allocation has no source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stability {
    /// ⊥ no information (unreachable path / uninitialized).
    Bottom,
    /// Never changes: the same reference on every render (safe as a dep).
    Stable,
    /// Changes *only* at setter events of these state slots (may bound).
    /// Invariant: non-empty (canonicalised — `Versioned(∅) ≡ Stable`) and
    /// `len() ≤ VERSIONED_LABELS_THRESHOLD` (widened to `VersionedTop` above).
    Versioned(BTreeSet<QualifiedSlot>),
    /// Versioned by unknown state slots (threshold-widened `Versioned`).
    VersionedTop,
    /// A fresh reference every render, guaranteed (must bound).
    /// For non-reference kinds via `to_stability`: "may change every render".
    PerRender,
    /// No bound in either direction, on a value no allocation of this render
    /// reaches: what an opaque value hands back (a free name, an input, a
    /// call, a member read), which changes with what it is computed from.
    /// `top()`: what a name, a slot or a memo nothing binds reads as.
    Unknown,
    /// ⊤: a reference this render may have allocated on some path, anything
    /// on the others (`x ?? {}`, `c ? D : {}`): it may be new at a render
    /// where nothing it is computed from moved (#304).
    MayBeFresh,
}

impl Stability {
    /// Canonicalising constructor: ∅ → `Stable`, over-threshold → `VersionedTop`.
    pub fn versioned(labels: BTreeSet<QualifiedSlot>) -> Self {
        if labels.is_empty() {
            Stability::Stable
        } else if labels.len() > VERSIONED_LABELS_THRESHOLD {
            Stability::VersionedTop
        } else {
            Stability::Versioned(labels)
        }
    }

    /// Single-slot `Versioned`.
    pub fn versioned_by(component: ComponentId, label: HookLabel) -> Self {
        Stability::Versioned(BTreeSet::from([(component, label)]))
    }
}

impl PartialOrd for Stability {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use Stability::*;
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (Bottom, _) => Some(Ordering::Less),
            (_, Bottom) => Some(Ordering::Greater),
            (_, MayBeFresh) => Some(Ordering::Less),
            (MayBeFresh, _) => Some(Ordering::Greater),
            // PerRender vs every point between the ends: incomparable
            // (different bounds — may vs must; an allocation vs an input).
            (PerRender, _) | (_, PerRender) => None,
            (_, Unknown) => Some(Ordering::Less),
            (Unknown, _) => Some(Ordering::Greater),
            // Stable ⊑ Versioned ⊑ VersionedTop (behaviour-set inclusion:
            // "never changes" ⊂ "changes only at sets").
            (Stable, Versioned(_) | VersionedTop) => Some(Ordering::Less),
            (Versioned(_) | VersionedTop, Stable) => Some(Ordering::Greater),
            (Versioned(s), Versioned(t)) => {
                if s.is_subset(t) {
                    Some(Ordering::Less) // s ≠ t here (equal case above)
                } else if t.is_subset(s) {
                    Some(Ordering::Greater)
                } else {
                    None
                }
            }
            (Versioned(_), VersionedTop) => Some(Ordering::Less),
            (VersionedTop, Versioned(_)) => Some(Ordering::Greater),
            _ => None,
        }
    }
}

impl Stability {
    pub fn is_bottom(&self) -> bool {
        matches!(self, Stability::Bottom)
    }

    /// Least upper bound (⊔).
    pub fn join(&self, other: &Self) -> Self {
        use Stability::*;
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (Bottom, x) | (x, Bottom) => x.clone(),
            // A fresh arm joined with anything else may be new at a render
            // nothing it is computed from moved: `x ?? {}` (#304).
            (MayBeFresh, _) | (_, MayBeFresh) | (PerRender, _) | (_, PerRender) => MayBeFresh,
            (Unknown, _) | (_, Unknown) => Unknown,
            (Stable, v @ (Versioned(_) | VersionedTop))
            | (v @ (Versioned(_) | VersionedTop), Stable) => v.clone(),
            (Versioned(s), Versioned(t)) => Stability::versioned(s.union(t).cloned().collect()),
            (VersionedTop, Versioned(_)) | (Versioned(_), VersionedTop) => VersionedTop,
            (Stable, Stable) => Stable,
            (VersionedTop, VersionedTop) => VersionedTop,
        }
    }

    /// Greatest lower bound (⊓).
    pub fn meet(&self, other: &Self) -> Self {
        use Stability::*;
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (MayBeFresh, x) | (x, MayBeFresh) => x.clone(),
            (Bottom, _) | (_, Bottom) => Bottom,
            // PerRender ⊓ anything between the ends = Bottom
            (PerRender, _) | (_, PerRender) => Bottom,
            (Unknown, x) | (x, Unknown) => x.clone(),
            (Stable, Versioned(_) | VersionedTop) | (Versioned(_) | VersionedTop, Stable) => Stable,
            (Versioned(s), Versioned(t)) => {
                let inter: BTreeSet<_> = s.intersection(t).cloned().collect();
                Stability::versioned(inter) // ∅ canonicalises to Stable
            }
            (VersionedTop, v @ Versioned(_)) | (v @ Versioned(_), VersionedTop) => v.clone(),
            (Stable, Stable) => Stable,
            (VersionedTop, VersionedTop) => VersionedTop,
        }
    }

    /// Widening: join, whose `Versioned` union is already threshold-bounded —
    /// chains have height ≤ threshold + 5.
    pub fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }

    /// The bound of a value a condition selects (#197): `self` is the join
    /// of the arms, `cond` the may bound of the condition's own change trace.
    ///
    /// A selection `c ? a : b` changes whenever the chosen arm changes *or
    /// the choice flips*: `Δ(v) ⊆ Δ(a) ∪ Δ(b) ∪ Δ(c)`. So the may side of
    /// the result is `a ⊔ b ⊔ may(c)` — two `Stable` refs selected by a
    /// state boolean are `Versioned` by that state, not `Stable`. The must
    /// side is that of `a ⊔ b` alone: two arms that are each fresh every
    /// render give a fresh selection, so `PerRender` stays `PerRender`
    /// (a plain join with `may(c)` would weaken it to `MayBeFresh`). `Unknown`
    /// and `MayBeFresh` cannot grow by a condition's bound and `Bottom` is
    /// unreachable code.
    pub fn under_condition(&self, cond: &Self) -> Self {
        match self {
            Stability::Bottom
            | Stability::PerRender
            | Stability::Unknown
            | Stability::MayBeFresh => self.clone(),
            _ => self.join(cond),
        }
    }
}

impl AbstractDomain for Stability {
    fn bottom() -> Self {
        Stability::Bottom
    }
    /// What nothing binds reads as: an opaque value, `Unknown`, as
    /// [`StateValue::top`](crate::domains::StateValue) — not the lattice's ⊤,
    /// which only a join with an allocation reaches.
    fn top() -> Self {
        Stability::Unknown
    }
    fn is_bottom(&self) -> bool {
        self.is_bottom()
    }
    fn join(&self, other: &Self) -> Self {
        self.join(other)
    }
    fn meet(&self, other: &Self) -> Self {
        self.meet(other)
    }
    fn widen(&self, other: &Self) -> Self {
        self.widen(other)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn v(labels: &[(&str, HookLabel)]) -> Stability {
        Stability::Versioned(
            labels
                .iter()
                .map(|(c, l)| (crate::test_support::named(c), *l))
                .collect(),
        )
    }

    fn all_points() -> Vec<Stability> {
        vec![
            Stability::Bottom,
            Stability::Stable,
            v(&[("A", 0)]),
            v(&[("A", 0), ("B", 1)]),
            Stability::VersionedTop,
            Stability::PerRender,
            Stability::Unknown,
            Stability::MayBeFresh,
        ]
    }

    // ── join ──────────────────────────────────────────────────────────────────

    #[test]
    fn join_stable_perrender_may_be_fresh() {
        assert_eq!(
            Stability::Stable.join(&Stability::PerRender),
            Stability::MayBeFresh
        );
        assert_eq!(
            Stability::PerRender.join(&Stability::Stable),
            Stability::MayBeFresh
        );
    }

    #[test]
    fn join_versioned_perrender_may_be_fresh() {
        assert_eq!(
            v(&[("A", 0)]).join(&Stability::PerRender),
            Stability::MayBeFresh
        );
        assert_eq!(
            Stability::VersionedTop.join(&Stability::PerRender),
            Stability::MayBeFresh
        );
    }

    /// #304: an opaque value or a fresh allocation (`absent ?? {}`) may be new
    /// at a render nothing it is computed from moved: the join must not
    /// collapse into `Unknown`, which the re-run class reads through sources.
    #[test]
    fn join_unknown_perrender_may_be_fresh() {
        assert_eq!(
            Stability::Unknown.join(&Stability::PerRender),
            Stability::MayBeFresh
        );
        assert!(Stability::Unknown < Stability::MayBeFresh);
        assert!(
            Stability::Unknown
                .partial_cmp(&Stability::PerRender)
                .is_none()
        );
    }

    #[test]
    fn join_stable_versioned_keeps_versioned() {
        assert_eq!(Stability::Stable.join(&v(&[("A", 0)])), v(&[("A", 0)]));
        assert_eq!(v(&[("A", 0)]).join(&Stability::Stable), v(&[("A", 0)]));
    }

    #[test]
    fn join_versioned_is_label_union() {
        assert_eq!(
            v(&[("A", 0)]).join(&v(&[("B", 1)])),
            v(&[("A", 0), ("B", 1)])
        );
    }

    #[test]
    fn join_versioned_over_threshold_widens_to_top() {
        let big = v(&[("A", 0), ("A", 1), ("A", 2), ("A", 3)]);
        assert_eq!(big.join(&v(&[("B", 9)])), Stability::VersionedTop);
    }

    #[test]
    fn join_with_bottom_is_identity() {
        for x in all_points() {
            assert_eq!(Stability::Bottom.join(&x), x);
            assert_eq!(x.join(&Stability::Bottom), x);
        }
    }

    #[test]
    fn join_with_unknown_is_unknown_unless_fresh() {
        for x in all_points() {
            let fresh = matches!(x, Stability::PerRender | Stability::MayBeFresh);
            assert_eq!(
                x.join(&Stability::Unknown),
                if fresh {
                    Stability::MayBeFresh
                } else {
                    Stability::Unknown
                }
            );
            assert_eq!(x.join(&Stability::MayBeFresh), Stability::MayBeFresh);
        }
    }

    #[test]
    fn join_idempotent_commutative() {
        for a in all_points() {
            assert_eq!(a.join(&a), a);
            for b in all_points() {
                assert_eq!(a.join(&b), b.join(&a), "join({a:?},{b:?}) not commutative");
            }
        }
    }

    #[test]
    fn join_is_upper_bound() {
        for a in all_points() {
            for b in all_points() {
                let j = a.join(&b);
                assert!(a <= j, "{a:?} ⋢ join({a:?},{b:?})={j:?}");
                assert!(b <= j, "{b:?} ⋢ join({a:?},{b:?})={j:?}");
            }
        }
    }

    // ── under_condition (#197) ───────────────────────────────────────────────

    #[test]
    fn under_condition_two_stable_arms_take_the_condition() {
        // `flag ? r1 : r2` with two refs: the selection changes with `flag`.
        assert_eq!(
            Stability::Stable.under_condition(&v(&[("A", 0)])),
            v(&[("A", 0)])
        );
        assert_eq!(
            v(&[("A", 1)]).under_condition(&v(&[("A", 0)])),
            v(&[("A", 0), ("A", 1)])
        );
        assert_eq!(
            Stability::Stable.under_condition(&Stability::Unknown),
            Stability::Unknown
        );
    }

    #[test]
    fn under_condition_keeps_the_must_side_of_fresh_arms() {
        // Both arms fresh ⇒ the selection is fresh: PerRender is not weakened.
        for c in all_points() {
            assert_eq!(
                Stability::PerRender.under_condition(&c),
                Stability::PerRender
            );
            assert_eq!(Stability::Unknown.under_condition(&c), Stability::Unknown);
            assert_eq!(
                Stability::MayBeFresh.under_condition(&c),
                Stability::MayBeFresh
            );
            assert_eq!(Stability::Bottom.under_condition(&c), Stability::Bottom);
        }
    }

    #[test]
    fn under_condition_is_extensive_and_trivial_for_a_constant() {
        for a in all_points() {
            for c in all_points() {
                let r = a.under_condition(&c);
                assert!(a <= r, "{a:?} ⋢ under_condition({a:?},{c:?})={r:?}");
            }
            assert_eq!(a.under_condition(&Stability::Stable), a);
            assert_eq!(a.under_condition(&Stability::Bottom), a);
        }
    }

    // ── canonicalisation ─────────────────────────────────────────────────────

    #[test]
    fn versioned_empty_is_stable() {
        assert_eq!(Stability::versioned(BTreeSet::new()), Stability::Stable);
    }

    #[test]
    fn meet_disjoint_versioned_is_stable() {
        // intersection ∅ → canonicalises to Stable ("versioned by nothing").
        assert_eq!(v(&[("A", 0)]).meet(&v(&[("B", 1)])), Stability::Stable);
    }

    // ── meet ─────────────────────────────────────────────────────────────────

    #[test]
    fn meet_stable_perrender_is_bottom() {
        assert_eq!(
            Stability::Stable.meet(&Stability::PerRender),
            Stability::Bottom
        );
    }

    #[test]
    fn meet_versioned_is_label_intersection() {
        assert_eq!(
            v(&[("A", 0), ("B", 1)]).meet(&v(&[("B", 1), ("C", 2)])),
            v(&[("B", 1)])
        );
    }

    #[test]
    fn meet_with_the_top_is_identity() {
        for x in all_points() {
            assert_eq!(x.meet(&Stability::MayBeFresh), x);
        }
    }

    #[test]
    fn meet_is_lower_bound() {
        for a in all_points() {
            for b in all_points() {
                let m = a.meet(&b);
                assert!(m <= a, "meet({a:?},{b:?})={m:?} ⋢ {a:?}");
                assert!(m <= b, "meet({a:?},{b:?})={m:?} ⋢ {b:?}");
            }
        }
    }

    // ── partial order ─────────────────────────────────────────────────────────

    #[test]
    fn bottom_least_may_be_fresh_greatest() {
        for x in all_points() {
            assert!(Stability::Bottom <= x);
            assert!(x <= Stability::MayBeFresh);
        }
    }

    #[test]
    fn stable_below_versioned_below_versioned_top() {
        assert!(Stability::Stable <= v(&[("A", 0)]));
        assert!(v(&[("A", 0)]) <= v(&[("A", 0), ("B", 1)]));
        assert!(v(&[("A", 0)]) <= Stability::VersionedTop);
    }

    #[test]
    fn perrender_incomparable_with_stable_and_versioned() {
        for x in [Stability::Stable, v(&[("A", 0)]), Stability::VersionedTop] {
            assert!(x.partial_cmp(&Stability::PerRender).is_none());
        }
    }

    #[test]
    fn incomparable_versioned_sets() {
        assert!(v(&[("A", 0)]).partial_cmp(&v(&[("B", 1)])).is_none());
    }

    // ── widen = join (threshold already inside join) ──────────────────────────

    #[test]
    fn widen_equals_join() {
        for a in all_points() {
            for b in all_points() {
                assert_eq!(a.widen(&b), a.join(&b), "widen({a:?}, {b:?}) ≠ join");
            }
        }
    }
}
