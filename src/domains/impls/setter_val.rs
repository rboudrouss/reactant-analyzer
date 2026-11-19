use std::collections::BTreeSet;

use crate::ir::{ComponentId, types::HookLabel};

/// The setters a value may be: the powerset of the program's setter
/// identities, open or closed to the setters the analysis cannot name.
///
/// ```text
///                         Top   (any setter: the bound, no transfer yields it)
///                          |
///       Open{ a, b }  Open{ a, c } ...
///            |      ×      |
///   { a, b } Open{ a } Open{ b } ...
///        |   ×   |  ×   |
///      { a }    { b }  Open{}  (a setter the analysis does not name)
///          \      |     /
///           Bottom  (not a setter)
/// ```
///
/// A join keeps every candidate: a prop one call site binds to `setA` and
/// another to `setB` writes one of the two, and both are writes the
/// analysis must see (#175, #202). A value the analysis cannot see into (an
/// unknown callee's result, ⊤) may be a setter it does not name, which must
/// not erase the ones it does: `setV` joined with ⊤ is `Open{setV}`, still a
/// possible write of `setV` (#238). React guarantees setter identity across
/// renders, so any non-bottom value maps to `Stability::Stable`.
///
/// **No cap on the names (#329).** A join keeps every name, however many: a
/// call of the value writes each, and a name dropped would be a write the
/// analysis misses (a 9-setter `fs[i](n + 1)` used to widen to ⊤ and write
/// nothing). The height stays finite without a cap: every setter is born
/// named (`StateSetter` → [`SetterVal::one`]), one per `useState` /
/// `useReducer` slot of the program, so a set holds at most the program's
/// slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetterVal {
    /// ⊥ — no setter value possible.
    Bottom,
    /// One of these setters — never empty.
    Some(BTreeSet<(ComponentId, HookLabel)>),
    /// One of these setters or one the analysis does not name — empty for a
    /// value nothing named.
    Open(BTreeSet<(ComponentId, HookLabel)>),
    /// ⊤ — any setter of the program. The lattice's bound for
    /// [`AbstractDomain::top`](crate::domains::AbstractDomain::top) only: no
    /// transfer produces it (an unknown value is [`SetterVal::unnamed`], a
    /// join of named setters keeps the names), and a call of it would have
    /// to write every slot of the program, which nothing enumerates.
    Top,
}

impl SetterVal {
    /// Exactly this component's setter for this hook label.
    pub fn one(component: ComponentId, label: HookLabel) -> Self {
        SetterVal::Some(BTreeSet::from([(component, label)]))
    }

    /// A setter the analysis does not name, if a setter at all: the slot of
    /// a value it cannot see into.
    pub fn unnamed() -> Self {
        SetterVal::Open(BTreeSet::new())
    }

    /// Payload accessor: `Some` only when the setter identity is exact.
    pub fn as_one(&self) -> Option<(&ComponentId, &HookLabel)> {
        match self {
            SetterVal::Some(s) if s.len() == 1 => s.first().map(|(c, l)| (c, l)),
            _ => None,
        }
    }

    /// Every setter the value may be that the analysis names; empty for ⊥
    /// and for ⊤, which no transfer produces ([`SetterVal::Top`]).
    pub fn candidates(&self) -> impl Iterator<Item = (ComponentId, HookLabel)> + '_ {
        debug_assert!(
            *self != SetterVal::Top,
            "no transfer produces the setter ⊤: its names are what a call writes"
        );
        match self {
            SetterVal::Some(s) | SetterVal::Open(s) => Some(s.iter().copied()),
            _ => None,
        }
        .into_iter()
        .flatten()
    }

    /// The value, if a setter, is one of [`Self::candidates`].
    pub fn is_closed(&self) -> bool {
        matches!(self, SetterVal::Bottom | SetterVal::Some(_))
    }

    /// The names and whether the set is open; ⊤ has none.
    fn parts(&self) -> Option<(&BTreeSet<(ComponentId, HookLabel)>, bool)> {
        static EMPTY: BTreeSet<(ComponentId, HookLabel)> = BTreeSet::new();
        match self {
            SetterVal::Bottom => Some((&EMPTY, false)),
            SetterVal::Some(s) => Some((s, false)),
            SetterVal::Open(s) => Some((s, true)),
            SetterVal::Top => None,
        }
    }

    /// The element with these names, open or not.
    fn of(names: BTreeSet<(ComponentId, HookLabel)>, open: bool) -> Self {
        match (names.len(), open) {
            (_, true) => SetterVal::Open(names),
            (0, false) => SetterVal::Bottom,
            (_, false) => SetterVal::Some(names),
        }
    }
}

impl PartialOrd for SetterVal {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        if self == other {
            return Some(Ordering::Equal);
        }
        let leq = |(a, oa): (&BTreeSet<_>, bool), (b, ob): (&BTreeSet<_>, bool)| {
            a.is_subset(b) && (!oa || ob)
        };
        match (self.parts(), other.parts()) {
            (_, None) => Some(Ordering::Less),
            (None, _) => Some(Ordering::Greater),
            (Some(a), Some(b)) if leq(a, b) => Some(Ordering::Less),
            (Some(a), Some(b)) if leq(b, a) => Some(Ordering::Greater),
            _ => None,
        }
    }
}

impl crate::domains::AbstractDomain for SetterVal {
    fn bottom() -> Self {
        SetterVal::Bottom
    }
    fn top() -> Self {
        SetterVal::Top
    }
    fn is_bottom(&self) -> bool {
        matches!(self, SetterVal::Bottom)
    }
    fn join(&self, other: &Self) -> Self {
        match (self.parts(), other.parts()) {
            (Some((a, oa)), Some((b, ob))) => Self::of(a.union(b).copied().collect(), oa || ob),
            _ => SetterVal::Top,
        }
    }
    /// An open side constrains nothing it does not name (a setter it does
    /// not name may be any), so it leaves the other side's names; two open
    /// sides keep every name either has.
    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (SetterVal::Top, x) | (x, SetterVal::Top) => x.clone(),
            (SetterVal::Bottom, _) | (_, SetterVal::Bottom) => SetterVal::Bottom,
            (SetterVal::Open(_), x @ SetterVal::Some(_))
            | (x @ SetterVal::Some(_), SetterVal::Open(_)) => x.clone(),
            (SetterVal::Open(a), SetterVal::Open(b)) => {
                Self::of(a.union(b).copied().collect(), true)
            }
            (SetterVal::Some(a), SetterVal::Some(b)) => {
                Self::of(a.intersection(b).copied().collect(), false)
            }
        }
    }
    /// The lattice has finite height (the program's slots are finite):
    /// widening is the join.
    fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::AbstractDomain;

    fn one(c: &str, l: usize) -> SetterVal {
        SetterVal::one(crate::test_support::named(c), l)
    }

    #[test]
    fn join_same_is_identity() {
        assert_eq!(one("Foo", 0).join(&one("Foo", 0)), one("Foo", 0));
    }

    #[test]
    fn join_different_keeps_both() {
        let j = one("Foo", 0).join(&one("Bar", 0));
        assert_eq!(j.candidates().count(), 2);
        assert!(j.as_one().is_none());
        assert!(one("Foo", 0) < j && one("Bar", 0) < j);
    }

    /// #329: a join of any number of setters names every one of them — a
    /// call of it may write each.
    #[test]
    fn join_of_many_setters_keeps_every_name() {
        let j = (0..12)
            .map(|l| one("Foo", l))
            .reduce(|a, b| a.join(&b))
            .unwrap();
        assert_eq!(j.candidates().count(), 12);
        assert!(j.is_closed() && j < SetterVal::Top);
        let open = j.join(&SetterVal::unnamed());
        assert_eq!(open.candidates().count(), 12);
        assert!(!open.is_closed());
    }

    /// #238: a setter joined with a value the analysis cannot name stays a
    /// candidate, open to the others.
    #[test]
    fn join_with_unnamed_keeps_the_names() {
        let j = one("Foo", 0).join(&SetterVal::unnamed());
        assert_eq!(j.candidates().count(), 1);
        assert!(!j.is_closed());
        assert!(j.as_one().is_none());
        assert!(one("Foo", 0) < j && SetterVal::unnamed() < j && j < SetterVal::Top);
        assert_eq!(j.join(&SetterVal::unnamed()), j);
    }

    #[test]
    fn meet_different_is_bottom() {
        assert_eq!(one("Foo", 0).meet(&one("Bar", 0)), SetterVal::Bottom);
    }

    #[test]
    fn meet_with_unnamed_keeps_the_closed_side() {
        assert_eq!(one("Foo", 0).meet(&SetterVal::unnamed()), one("Foo", 0));
    }

    #[test]
    fn partial_ord() {
        assert!(SetterVal::Bottom < one("Foo", 0));
        assert!(one("Foo", 0) < SetterVal::Top);
        assert!(one("Foo", 0).partial_cmp(&one("Bar", 0)).is_none());
        assert!(one("Foo", 0).partial_cmp(&SetterVal::unnamed()).is_none());
    }
}
