use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::domains::AbstractDomain;

/// Max strings tracked before widening to Top.
const STR_WIDEN_THRESHOLD: usize = 4;

/// Abstract string-constant domain: a finite powerset lattice with widening.
///
/// Bottom ≤ Set(s) ≤ Top. Set(a) ≤ Set(b) iff a ⊆ b.
/// join(Set(a), Set(b)) widens to Top when |a ∪ b| > STR_WIDEN_THRESHOLD.
#[derive(Debug, Clone, PartialEq)]
pub enum StrConst {
    /// ⊥ — no possible string value (unreachable path).
    Bottom,
    /// Finite known set of string constants.
    Set(Arc<BTreeSet<String>>),
    /// ⊤ — any string (widened beyond threshold).
    Top,
}

impl StrConst {
    pub fn singleton(s: String) -> Self {
        let mut set = BTreeSet::new();
        set.insert(s);
        StrConst::Set(Arc::new(set))
    }

    fn from_set(set: BTreeSet<String>) -> Self {
        if set.is_empty() {
            StrConst::Bottom
        } else if set.len() > STR_WIDEN_THRESHOLD {
            StrConst::Top
        } else {
            StrConst::Set(Arc::new(set))
        }
    }
}

impl PartialOrd for StrConst {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (StrConst::Bottom, _) => Some(Ordering::Less),
            (_, StrConst::Bottom) => Some(Ordering::Greater),
            (_, StrConst::Top) => Some(Ordering::Less),
            (StrConst::Top, _) => Some(Ordering::Greater),
            (StrConst::Set(a), StrConst::Set(b)) => {
                let a_sub_b = a.is_subset(b);
                let b_sub_a = b.is_subset(a);
                match (a_sub_b, b_sub_a) {
                    (true, true) => Some(Ordering::Equal),
                    (true, false) => Some(Ordering::Less),
                    (false, true) => Some(Ordering::Greater),
                    (false, false) => None,
                }
            }
        }
    }
}

impl AbstractDomain for StrConst {
    fn bottom() -> Self {
        StrConst::Bottom
    }
    fn top() -> Self {
        StrConst::Top
    }
    fn is_bottom(&self) -> bool {
        matches!(self, StrConst::Bottom)
    }

    fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StrConst::Bottom, x) | (x, StrConst::Bottom) => x.clone(),
            (StrConst::Top, _) | (_, StrConst::Top) => StrConst::Top,
            (StrConst::Set(a), StrConst::Set(b)) => {
                let union: BTreeSet<String> = a.iter().cloned().chain(b.iter().cloned()).collect();
                StrConst::from_set(union)
            }
        }
    }

    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StrConst::Bottom, _) | (_, StrConst::Bottom) => StrConst::Bottom,
            (StrConst::Top, x) | (x, StrConst::Top) => x.clone(),
            (StrConst::Set(a), StrConst::Set(b)) => {
                let inter: BTreeSet<String> = a.intersection(b).cloned().collect();
                StrConst::from_set(inter)
            }
        }
    }

    fn widen(&self, other: &Self) -> Self {
        // join already applies the threshold — widening = join for this domain
        self.join(other)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn set(strs: &[&str]) -> StrConst {
        StrConst::Set(Arc::new(strs.iter().map(|s| s.to_string()).collect()))
    }

    #[test]
    fn singleton_is_not_top_or_bottom() {
        let s = StrConst::singleton("hello".to_string());
        assert!(!s.is_bottom());
        assert_ne!(s, StrConst::Top);
    }

    #[test]
    fn join_two_singletons_gives_pair() {
        let a = StrConst::singleton("a".to_string());
        let b = StrConst::singleton("b".to_string());
        assert_eq!(a.join(&b), set(&["a", "b"]));
    }

    #[test]
    fn join_beyond_threshold_widens_to_top() {
        let mut v = StrConst::singleton("a".to_string());
        for c in ["b", "c", "d", "e"] {
            v = v.join(&StrConst::singleton(c.to_string()));
        }
        assert_eq!(v, StrConst::Top);
    }

    #[test]
    fn meet_gives_intersection() {
        let a = set(&["x", "y"]);
        let b = set(&["y", "z"]);
        assert_eq!(a.meet(&b), StrConst::singleton("y".to_string()));
    }

    #[test]
    fn partial_ord_subset() {
        let single = StrConst::singleton("a".to_string());
        let pair = set(&["a", "b"]);
        assert!(single < pair);
        assert!(!(pair < single));
        assert!(single <= StrConst::Top);
        assert!(StrConst::Bottom <= single);
    }
}
