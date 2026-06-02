use std::cmp::Ordering;

use crate::{domains::AbstractDomain, ir::expr::Expr};

/// Stability lattice — tracks whether a value's reference is stable across renders.
///
/// ```text
///        Unknown  (⊤)
///        /     \
///   Stable   Unstable
///        \     /
///         Bottom  (⊥)
/// ```
///
/// `Stable` and `Unstable` are incomparable (neither implies the other).
/// `join(Stable, Unstable) = Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// ⊥ — no information (unreachable path / uninitialized).
    Bottom,
    /// Reference is the same object on every render (safe as a dep).
    Stable,
    /// Reference changes on every render (unsafe as a dep if not memoized).
    Unstable,
    /// ⊤ — may be either stable or unstable (join of both paths).
    Unknown,
}

impl PartialOrd for Stability {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (Stability::Bottom, _) => Some(Ordering::Less),
            (_, Stability::Unknown) => Some(Ordering::Less),
            (_, Stability::Bottom) => Some(Ordering::Greater),
            (Stability::Unknown, _) => Some(Ordering::Greater),
            _ => None, // Stable vs Unstable: incomparable
        }
    }
}

impl Stability {
    pub fn is_bottom(&self) -> bool {
        matches!(self, Stability::Bottom)
    }

    /// Least upper bound (⊔).
    pub fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => *a,
            (Stability::Bottom, x) | (x, Stability::Bottom) => *x,
            _ => Stability::Unknown,
        }
    }

    /// Greatest lower bound (⊓).
    pub fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => *a,
            (Stability::Unknown, x) | (x, Stability::Unknown) => *x,
            _ => Stability::Bottom,
        }
    }

    /// Widening — equals join for this finite-height lattice (height 2).
    pub fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }
}

impl AbstractDomain for Stability {
    fn bottom() -> Self {
        Stability::Bottom
    }
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

impl Stability {
    /// Static stability of an expression, without an environment.
    /// Used as a fast path when the value is structurally determined.
    pub fn from_expr_static(expr: &Expr) -> Stability {
        match expr {
            Expr::Lit(_) => Stability::Stable,
            Expr::ObjectLit(_) => Stability::Unstable,
            Expr::ArrayLit(_) => Stability::Unstable,
            Expr::FnLit { .. } => Stability::Unstable,
            Expr::StateSetter(_) => Stability::Stable,
            _ => Stability::Unknown,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::{Expr, Prim};

    // ── join ──────────────────────────────────────────────────────────────────

    #[test]
    fn join_stable_unstable_is_unknown() {
        assert_eq!(
            Stability::Stable.join(&Stability::Unstable),
            Stability::Unknown
        );
        assert_eq!(
            Stability::Unstable.join(&Stability::Stable),
            Stability::Unknown
        );
    }

    #[test]
    fn join_with_bottom_is_identity() {
        assert_eq!(
            Stability::Stable.join(&Stability::Bottom),
            Stability::Stable
        );
        assert_eq!(
            Stability::Unstable.join(&Stability::Bottom),
            Stability::Unstable
        );
        assert_eq!(
            Stability::Bottom.join(&Stability::Stable),
            Stability::Stable
        );
    }

    #[test]
    fn join_with_unknown_is_unknown() {
        assert_eq!(
            Stability::Stable.join(&Stability::Unknown),
            Stability::Unknown
        );
        assert_eq!(
            Stability::Unstable.join(&Stability::Unknown),
            Stability::Unknown
        );
        assert_eq!(
            Stability::Bottom.join(&Stability::Unknown),
            Stability::Unknown
        );
    }

    #[test]
    fn join_idempotent() {
        for v in [
            Stability::Bottom,
            Stability::Stable,
            Stability::Unstable,
            Stability::Unknown,
        ] {
            assert_eq!(v.join(&v), v);
        }
    }

    // ── meet ─────────────────────────────────────────────────────────────────

    #[test]
    fn meet_stable_unstable_is_bottom() {
        assert_eq!(
            Stability::Stable.meet(&Stability::Unstable),
            Stability::Bottom
        );
    }

    #[test]
    fn meet_with_unknown_is_identity() {
        assert_eq!(
            Stability::Stable.meet(&Stability::Unknown),
            Stability::Stable
        );
        assert_eq!(
            Stability::Unstable.meet(&Stability::Unknown),
            Stability::Unstable
        );
    }

    // ── partial order ─────────────────────────────────────────────────────────

    #[test]
    fn bottom_is_least() {
        assert!(Stability::Bottom <= Stability::Bottom);
        assert!(Stability::Bottom <= Stability::Stable);
        assert!(Stability::Bottom <= Stability::Unstable);
        assert!(Stability::Bottom <= Stability::Unknown);
    }

    #[test]
    fn unknown_is_greatest() {
        assert!(Stability::Stable <= Stability::Unknown);
        assert!(Stability::Unstable <= Stability::Unknown);
        assert!(Stability::Unknown <= Stability::Unknown);
    }

    #[test]
    fn stable_and_unstable_are_incomparable() {
        assert!(!(Stability::Stable <= Stability::Unstable));
        assert!(!(Stability::Unstable <= Stability::Stable));
    }

    // ── widen = join ──────────────────────────────────────────────────────────

    #[test]
    fn widen_equals_join() {
        let pairs = [
            (Stability::Bottom, Stability::Stable),
            (Stability::Stable, Stability::Unstable),
            (Stability::Unstable, Stability::Unknown),
            (Stability::Unknown, Stability::Bottom),
        ];
        for (a, b) in pairs {
            assert_eq!(a.widen(&b), a.join(&b), "widen({a:?}, {b:?}) ≠ join");
        }
    }

    // ── from_expr_static ──────────────────────────────────────────────────────

    #[test]
    fn literal_is_stable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::Lit(Prim::Int(0))),
            Stability::Stable
        );
        assert_eq!(
            Stability::from_expr_static(&Expr::Lit(Prim::Bool(true))),
            Stability::Stable
        );
        assert_eq!(
            Stability::from_expr_static(&Expr::Lit(Prim::Null)),
            Stability::Stable
        );
    }

    #[test]
    fn object_lit_is_unstable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::ObjectLit(vec![])),
            Stability::Unstable
        );
    }

    #[test]
    fn array_lit_is_unstable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::ArrayLit(vec![])),
            Stability::Unstable
        );
    }

    #[test]
    fn fn_lit_is_unstable() {
        use crate::ir::cfg::{BasicBlock, CFG, Terminator};
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Unreachable,
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };
        assert_eq!(
            Stability::from_expr_static(&Expr::FnLit {
                params: vec![],
                body_cfg: Box::new(cfg)
            }),
            Stability::Unstable
        );
    }

    #[test]
    fn state_setter_is_stable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::StateSetter(0)),
            Stability::Stable
        );
        assert_eq!(
            Stability::from_expr_static(&Expr::StateSetter(99)),
            Stability::Stable
        );
    }

    #[test]
    fn var_is_unknown() {
        assert_eq!(
            Stability::from_expr_static(&Expr::Var("x".to_string())),
            Stability::Unknown
        );
    }
}
