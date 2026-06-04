pub type Symbol = String;
pub type HookLabel = usize;
pub type BlockId = usize;
pub type Var = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
