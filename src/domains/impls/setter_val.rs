use crate::ir::{ComponentId, types::HookLabel};

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
    One(ComponentId, HookLabel),
    /// ⊤ — some setter, but which one was lost at a join.
    Top,
}

impl SetterVal {
    /// Payload accessor: `Some` only when the setter identity is exact.
    pub fn as_one(&self) -> Option<(&ComponentId, &HookLabel)> {
        match self {
            SetterVal::One(c, l) => Some((c, l)),
            _ => None,
        }
    }
}

// Flat lattice: `⊥ < One(..) < ⊤`, distinct `One(..)` incomparable. React
// guarantees setter identity across renders, so any non-⊥ value is `Stable`.
flat_lattice!(SetterVal, bottom = Bottom, top = Top);

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::AbstractDomain;

    fn one(c: &str, l: usize) -> SetterVal {
        SetterVal::One(crate::test_support::named(c), l)
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
