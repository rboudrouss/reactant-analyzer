use std::collections::HashMap;

use crate::{domains::AbstractDomain, ir::types::HookLabel};

/// Maps each `useMemo` / `useCallback` hook label to its current abstract value.
///
/// Unlike `StateStore`, this store is NOT a fixpoint subject it is fully
/// recomputed via `Transfer::recompute_memo` after each render-pass analysis.
/// The `set` method is the only mutation; the recomputation logic lives in the
/// `Transfer` implementation so each domain can define its own semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoStore<D: AbstractDomain>(HashMap<HookLabel, D>);

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
        self.0.get(&label).cloned().unwrap_or_else(D::top)
    }

    /// Store a precomputed domain value for a label.
    pub fn set(&mut self, label: HookLabel, val: D) {
        self.0.insert(label, val);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::Stability;

    type Memo = MemoStore<Stability>;

    #[test]
    fn get_unknown_label_is_top() {
        assert_eq!(Memo::new().get(0), Stability::Unknown);
    }

    #[test]
    fn set_and_get() {
        let mut m = Memo::new();
        m.set(0, Stability::Stable);
        assert_eq!(m.get(0), Stability::Stable);
        m.set(0, Stability::PerRender);
        assert_eq!(m.get(0), Stability::PerRender);
    }

    #[test]
    fn multiple_labels_independent() {
        let mut m = Memo::new();
        m.set(0, Stability::Stable);
        m.set(1, Stability::PerRender);
        assert_eq!(m.get(0), Stability::Stable);
        assert_eq!(m.get(1), Stability::PerRender);
        assert_eq!(m.get(2), Stability::Unknown); // unset → top
    }
}
