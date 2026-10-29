pub type Symbol = String;
pub type HookLabel = usize;
pub type BlockId = usize;
pub type Var = String;

/// A state slot qualified by the component that owns it. `HookLabel` is
/// per-component, so every cross-component fact — a `Versioned` label set, a
/// churn node, a foreign writer row — names a slot this way (ADR-042 §2).
pub type QualifiedSlot = (super::component_id::ComponentId, HookLabel);

/// Allocation-site key. `Ord` so that a walk over a set of sites has one
/// stable order — the first match over a `HashSet<ExprId>` used to depend on
/// the process hash seed (#120).
///
/// A `u64`, not a `usize`: a component's sites live in a range of their own
/// ([`ComponentId::site_base`](crate::ir::ComponentId::site_base)), and the
/// layout must be the same on every target — the wasm build answers what the
/// native one does (#247).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprId(pub u64);

impl ExprId {
    /// Allocate a fresh `ExprId` not derived from any AST node.
    /// Used when creating synthetic heap values during inter-component analysis.
    pub fn fresh() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1_000_000_000);
        ExprId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}
