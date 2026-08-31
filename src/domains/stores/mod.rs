pub mod abstract_env;
pub mod heap;
pub mod memo_store;
pub mod shared_state_store;
pub mod state_store;

pub use abstract_env::{AbstractEnv, EnvVal};
pub use heap::{Heap, HeapValue, resolve_locs};
pub use memo_store::MemoStore;
pub use shared_state_store::SharedStateStore;
pub use state_store::StateStore;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::Hash;

use crate::domains::AbstractDomain;

/// `a ⊑ b` in a (possibly partial) lattice order: `Less`/`Equal` hold,
/// incomparable/`Greater` do not. Centralises the pointwise ≤ check the store
/// `leq`s share, spelled with `partial_cmp` (the form clippy prefers over a
/// negated comparison operator).
pub(crate) fn leq_pointwise<D: AbstractDomain>(a: &D, b: &D) -> bool {
    matches!(a.partial_cmp(b), Some(Ordering::Less | Ordering::Equal))
}

/// `m[k]` cloned, or `default()` when the key is absent — the get-or-default
/// lattice read every store performs (`⊥` or `⊤` for an unseen key).
pub(crate) fn map_get_or<K: Eq + Hash, D: Clone>(
    m: &HashMap<K, D>,
    k: &K,
    default: impl Fn() -> D,
) -> D {
    m.get(k).cloned().unwrap_or_else(default)
}
