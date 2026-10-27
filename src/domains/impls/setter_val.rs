use std::collections::BTreeSet;

use crate::ir::{ComponentId, types::HookLabel};

/// The setters a value may be: a bounded powerset of setter identities.
///
/// ```text
///                 Top   (some setter, more than MAX candidates)
///                  |
///     { a, b } { a, c } { b, c } ...
///        |    ×    |    ×   |
///      { a }     { b }    { c } ...
///          \       |       /
///           Bottom  (not a setter)
/// ```
///
/// A join keeps every candidate: a prop one call site binds to `setA` and
/// another to `setB` writes one of the two, and both are writes the
/// analysis must see (#175, #202). React guarantees setter identity across
/// renders, so any non-bottom value maps to `Stability::Stable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetterVal {
    /// ⊥ — no setter value possible.
    Bottom,
    /// One of these setters — never empty, at most [`SetterVal::MAX`].
    Some(BTreeSet<(ComponentId, HookLabel)>),
    /// ⊤ — some setter, which one the analysis no longer tracks.
    Top,
}

impl SetterVal {
    /// Past this many candidates a join answers ⊤: the lattice keeps a
    /// finite height.
    pub const MAX: usize = 8;

    /// Exactly this component's setter for this hook label.
    pub fn one(component: ComponentId, label: HookLabel) -> Self {
        SetterVal::Some(BTreeSet::from([(component, label)]))
    }

    /// Payload accessor: `Some` only when the setter identity is exact.
    pub fn as_one(&self) -> Option<(&ComponentId, &HookLabel)> {
        match self {
            SetterVal::Some(s) if s.len() == 1 => s.first().map(|(c, l)| (c, l)),
            _ => None,
        }
    }

    /// Every setter the value may be; empty for ⊥ and ⊤.
    pub fn candidates(&self) -> impl Iterator<Item = (ComponentId, HookLabel)> + '_ {
        match self {
            SetterVal::Some(s) => Some(s.iter().copied()),
            _ => None,
        }
        .into_iter()
        .flatten()
    }
}

impl PartialOrd for SetterVal {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (SetterVal::Bottom, _) | (_, SetterVal::Top) => Some(Ordering::Less),
            (SetterVal::Top, _) | (_, SetterVal::Bottom) => Some(Ordering::Greater),
            (SetterVal::Some(a), SetterVal::Some(b)) if a.is_subset(b) => Some(Ordering::Less),
            (SetterVal::Some(a), SetterVal::Some(b)) if b.is_subset(a) => Some(Ordering::Greater),
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
        match (self, other) {
            (SetterVal::Bottom, x) | (x, SetterVal::Bottom) => x.clone(),
            (SetterVal::Top, _) | (_, SetterVal::Top) => SetterVal::Top,
            (SetterVal::Some(a), SetterVal::Some(b)) => {
                let u: BTreeSet<_> = a.union(b).copied().collect();
                if u.len() > Self::MAX {
                    SetterVal::Top
                } else {
                    SetterVal::Some(u)
                }
            }
        }
    }
    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (SetterVal::Top, x) | (x, SetterVal::Top) => x.clone(),
            (SetterVal::Bottom, _) | (_, SetterVal::Bottom) => SetterVal::Bottom,
            (SetterVal::Some(a), SetterVal::Some(b)) => {
                let i: BTreeSet<_> = a.intersection(b).copied().collect();
                if i.is_empty() {
                    SetterVal::Bottom
                } else {
                    SetterVal::Some(i)
                }
            }
        }
    }
    /// The lattice has finite height: widening is the join.
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

    #[test]
    fn join_past_the_bound_is_top() {
        let j = (0..=SetterVal::MAX)
            .map(|l| one("Foo", l))
            .reduce(|a, b| a.join(&b))
            .unwrap();
        assert_eq!(j, SetterVal::Top);
    }

    #[test]
    fn meet_different_is_bottom() {
        assert_eq!(one("Foo", 0).meet(&one("Bar", 0)), SetterVal::Bottom);
    }

    #[test]
    fn partial_ord() {
        assert!(SetterVal::Bottom < one("Foo", 0));
        assert!(one("Foo", 0) < SetterVal::Top);
        assert!(one("Foo", 0).partial_cmp(&one("Bar", 0)).is_none());
    }
}
