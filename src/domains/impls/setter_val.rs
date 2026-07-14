use std::cmp::Ordering;

use crate::domains::AbstractDomain;
use crate::ir::types::{HookLabel, Symbol};

/// Flat lattice for the component-setter slot of `StateValue`.
///
/// ```text
///            Top   (some setter, identity unknown)
///          /  |  \
///   One(a) One(b) One(c) ...
///          \  |  /
///           Bottom  (not a setter)
/// ```
///
/// React guarantees setter identity across renders, so any non-bottom
/// value maps to `Stability::Stable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetterVal {
    /// ⊥ — no setter value possible.
    Bottom,
    /// Exactly this component's setter for this hook label.
    One(Symbol, HookLabel),
    /// ⊤ — some setter, but which one was lost at a join.
    Top,
}

impl SetterVal {
    /// Payload accessor: `Some` only when the setter identity is exact.
    pub fn as_one(&self) -> Option<(&Symbol, &HookLabel)> {
        match self {
            SetterVal::One(c, l) => Some((c, l)),
            _ => None,
        }
    }
}

impl PartialOrd for SetterVal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (SetterVal::Bottom, _) | (_, SetterVal::Top) => Some(Ordering::Less),
            (SetterVal::Top, _) | (_, SetterVal::Bottom) => Some(Ordering::Greater),
            _ => None, // two distinct One(..) are incomparable
        }
    }
}

impl AbstractDomain for SetterVal {
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
            (a, b) if a == b => a.clone(),
            (SetterVal::Bottom, x) | (x, SetterVal::Bottom) => x.clone(),
            _ => SetterVal::Top,
        }
    }

    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (SetterVal::Top, x) | (x, SetterVal::Top) => x.clone(),
            _ => SetterVal::Bottom,
        }
    }

    fn widen(&self, other: &Self) -> Self {
        // Finite-height lattice: widen = join.
        self.join(other)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn one(c: &str, l: usize) -> SetterVal {
        SetterVal::One(c.to_string(), l)
    }

    #[test]
    fn join_same_is_identity() {
        assert_eq!(one("Foo", 0).join(&one("Foo", 0)), one("Foo", 0));
    }

    #[test]
    fn join_different_is_top() {
        assert_eq!(one("Foo", 0).join(&one("Bar", 0)), SetterVal::Top);
        assert_eq!(one("Foo", 0).join(&one("Foo", 1)), SetterVal::Top);
    }

    #[test]
    fn meet_different_is_bottom() {
        assert_eq!(one("Foo", 0).meet(&one("Bar", 0)), SetterVal::Bottom);
    }

    #[test]
    fn partial_ord_flat() {
        assert!(SetterVal::Bottom < one("Foo", 0));
        assert!(one("Foo", 0) < SetterVal::Top);
        assert!(one("Foo", 0).partial_cmp(&one("Bar", 0)).is_none());
    }
}
