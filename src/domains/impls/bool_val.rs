use std::cmp::Ordering;

use crate::domains::AbstractDomain;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolVal {
    /// ⊥ — unreachable.
    Bottom,
    True,
    False,
    /// ⊤ — may be either.
    Top,
}

impl PartialOrd for BoolVal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (BoolVal::Bottom, _) | (_, BoolVal::Top) => Some(Ordering::Less),
            (BoolVal::Top, _) | (_, BoolVal::Bottom) => Some(Ordering::Greater),
            _ => None,
        }
    }
}

impl AbstractDomain for BoolVal {
    fn bottom() -> Self { BoolVal::Bottom }
    fn top() -> Self { BoolVal::Top }
    fn is_bottom(&self) -> bool { matches!(self, BoolVal::Bottom) }

    fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => *a,
            (BoolVal::Bottom, x) | (x, BoolVal::Bottom) => *x,
            _ => BoolVal::Top,
        }
    }

    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => *a,
            (BoolVal::Top, x) | (x, BoolVal::Top) => *x,
            _ => BoolVal::Bottom,
        }
    }

    fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }
}
