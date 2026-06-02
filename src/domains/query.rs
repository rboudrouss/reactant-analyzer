use crate::domains::{AbstractEnv, MemoStore, QueryContext, StateStore, Transfer};

// ── DomainQuery ───────────────────────────────────────────────────────────────

/// Marker trait for a cross-domain query. The associated `Result` type is the
/// answer returned by whichever domain handles the query.
///
/// This is the B1 generic manager foundation (see ADR-007). Because Rust lacks
/// OCaml GADTs, each query is a concrete struct; type-safety comes from the
/// `Queryable<Q>` bound on the answering transfer.
pub trait DomainQuery: 'static {
    type Result: 'static;
}

// ── Queryable ─────────────────────────────────────────────────────────────────

/// A `Transfer` that can answer queries of type `Q`.
///
/// Implement this on a transfer struct to expose typed cross-domain reads.
/// `ProductTransfer<T1, T2>` automatically delegates to whichever sub-transfer
/// implements `Queryable<Q>`.
pub trait Queryable<Q: DomainQuery>: Transfer {
    fn ask(
        &self,
        q: &Q,
        env: &AbstractEnv<Self::Domain>,
        state: &StateStore<Self::Domain>,
        memo: &MemoStore<Self::Domain>,
        ctx: &dyn QueryContext,
    ) -> Q::Result;
}
