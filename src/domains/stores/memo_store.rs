use std::collections::HashMap;

use crate::{domains::AbstractDomain, ir::types::HookLabel};

use super::{EnvVal, Sites};

/// Maps each `useMemo` / `useCallback` hook label to its current abstract value.
///
/// Unlike `StateStore`, this store is NOT a fixpoint subject it is fully
/// recomputed via `Transfer::recompute_memo` after each render-pass analysis.
/// The `set` method is the only mutation; the recomputation logic lives in the
/// `Transfer` implementation so each domain can define its own semantics.
///
/// A value keeps the allocation sites it may point to: a memo is the object
/// its body returned, whose members a caller reads through the heap (#233).
#[derive(Debug, Clone, PartialEq)]
pub struct MemoStore<D: AbstractDomain>(HashMap<HookLabel, EnvVal<D>>);

impl<D: AbstractDomain> Default for MemoStore<D> {
    fn default() -> Self {
        MemoStore(HashMap::new())
    }
}

impl<D: AbstractDomain> MemoStore<D> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `D::top()` for labels not yet computed (conservative).
    pub fn get(&self, label: HookLabel) -> D {
        self.0.get(&label).map_or_else(D::top, EnvVal::as_val)
    }

    /// The allocation sites the memo may point to, open when a recompute may
    /// hand back something no site places (#240); `None` when no recompute
    /// returns a site, or none ran yet.
    pub fn sites(&self, label: HookLabel) -> Option<&Sites> {
        self.0.get(&label).and_then(EnvVal::sites)
    }

    /// Store a precomputed value for a label.
    pub fn set(&mut self, label: HookLabel, val: EnvVal<D>) {
        self.0.insert(label, val);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::Stability;
    use crate::ir::types::ExprId;

    type Memo = MemoStore<Stability>;

    #[test]
    fn get_unknown_label_is_top() {
        assert_eq!(Memo::new().get(0), Stability::Unknown);
    }

    #[test]
    fn set_and_get() {
        let mut m = Memo::new();
        m.set(0, EnvVal::Val(Stability::Stable));
        assert_eq!(m.get(0), Stability::Stable);
        m.set(0, EnvVal::Val(Stability::PerRender));
        assert_eq!(m.get(0), Stability::PerRender);
        assert!(m.sites(0).is_none());
    }

    #[test]
    fn multiple_labels_independent() {
        let mut m = Memo::new();
        m.set(0, EnvVal::Val(Stability::Stable));
        m.set(1, EnvVal::Val(Stability::PerRender));
        assert_eq!(m.get(0), Stability::Stable);
        assert_eq!(m.get(1), Stability::PerRender);
        assert_eq!(m.get(2), Stability::Unknown); // unset → top
    }

    #[test]
    fn a_memo_keeps_the_sites_it_points_to() {
        let mut m = Memo::new();
        let sites = Sites::closed([ExprId(4)]);
        m.set(
            0,
            EnvVal::Loc {
                sites: sites.clone(),
                val: Stability::PerRender,
            },
        );
        assert_eq!(m.get(0), Stability::PerRender);
        assert_eq!(m.sites(0), Some(&sites));
    }
}
