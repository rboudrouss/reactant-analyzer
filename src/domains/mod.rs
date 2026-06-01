pub mod stability;

pub use stability::Stability;

/// Core abstract domain trait.
///
/// Lattice operations:
/// - `bottom()` = ⊥ (least element, "no information yet")
/// - `top()`    = ⊤ (greatest element, "all possible values")
/// - `join`     = ⊔ (least upper bound — used at merge points)
/// - `meet`     = ⊓ (greatest lower bound — used for refinement)
/// - `widen`    = convergence accelerator (= join for finite-height lattices)
///
/// `PartialOrd` represents the lattice order: `a ≤ b` iff `b` is "at least as
/// informative" as `a` (i.e. `a ⊑ b`).
pub trait AbstractDomain: Clone + PartialOrd {
    fn bottom() -> Self;
    fn top() -> Self;
    fn is_bottom(&self) -> bool;
    fn join(&self, other: &Self) -> Self;
    fn meet(&self, other: &Self) -> Self;
    fn widen(&self, other: &Self) -> Self;
}
