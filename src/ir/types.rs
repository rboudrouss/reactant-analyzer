pub type Symbol = String;
pub type HookLabel = usize;
pub type BlockId = usize;
pub type Var = String;

/// Allocation-site key. `Ord` so that a walk over a set of sites has one
/// stable order — the first match over a `HashSet<ExprId>` used to depend on
/// the process hash seed (#120).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprId(pub usize);

impl ExprId {
    /// Allocate a fresh `ExprId` not derived from any AST node.
    /// Used when creating synthetic heap values during inter-component analysis.
    pub fn fresh() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(1_000_000_000);
        ExprId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}
