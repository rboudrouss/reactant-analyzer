use std::cmp::Ordering;

use crate::domains::{
    AbstractDomain, AbstractEnv, AnalysisCtx, MemoStore, QueryContext, StateStore, Transfer,
};

use super::query::{DomainQuery, Queryable};

// ── ProductDomain ─────────────────────────────────────────────────────────────

/// Pointwise product of two abstract domains.
///
/// The lattice operations are applied independently to each component.
/// Used as the `Domain` associated type for `ProductTransfer`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProductDomain<D1: AbstractDomain, D2: AbstractDomain>(pub D1, pub D2);

impl<D1: AbstractDomain, D2: AbstractDomain> PartialOrd for ProductDomain<D1, D2> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let c1 = self.0.partial_cmp(&other.0)?;
        let c2 = self.1.partial_cmp(&other.1)?;
        match (c1, c2) {
            (Ordering::Equal, Ordering::Equal) => Some(Ordering::Equal),
            (Ordering::Less | Ordering::Equal, Ordering::Less | Ordering::Equal) => {
                Some(Ordering::Less)
            }
            (Ordering::Greater | Ordering::Equal, Ordering::Greater | Ordering::Equal) => {
                Some(Ordering::Greater)
            }
            _ => None,
        }
    }
}

impl<D1: AbstractDomain, D2: AbstractDomain> AbstractDomain for ProductDomain<D1, D2> {
    fn bottom() -> Self {
        ProductDomain(D1::bottom(), D2::bottom())
    }
    fn top() -> Self {
        ProductDomain(D1::top(), D2::top())
    }
    fn is_bottom(&self) -> bool {
        self.0.is_bottom() && self.1.is_bottom()
    }

    fn join(&self, other: &Self) -> Self {
        ProductDomain(self.0.join(&other.0), self.1.join(&other.1))
    }
    fn meet(&self, other: &Self) -> Self {
        ProductDomain(self.0.meet(&other.0), self.1.meet(&other.1))
    }
    fn widen(&self, other: &Self) -> Self {
        ProductDomain(self.0.widen(&other.0), self.1.widen(&other.1))
    }
}

// ── ProductTransfer ───────────────────────────────────────────────────────────

/// Combines two `Transfer` implementations into one product transfer.
///
/// **Current status**: groundwork for future fused fixpoints (e.g.
/// `Stability × StateValue` in the same pass). The `Transfer` implementation
/// is not yet wired into `analyze_component` that still uses
/// `StateValueTransfer` alone.
///
/// The primary purpose of this struct right now is enabling typed
/// `Queryable<Q>` delegation so that a composed transfer can answer
/// cross-domain queries from either sub-transfer.
///
/// Enables typed `Queryable<Q>` delegation from either sub-transfer.
pub struct ProductTransfer<T1, T2>(pub T1, pub T2);

// ── Queryable delegation ──────────────────────────────────────────────────────

/// Delegate `Queryable<Q>` to T1 when it handles the query type.
impl<T1, T2, Q> Queryable<Q> for ProductTransfer<T1, T2>
where
    T1: Transfer + Queryable<Q>,
    T2: Transfer,
    Q: DomainQuery,
    ProductDomain<T1::Domain, T2::Domain>: AbstractDomain,
{
    fn ask(
        &self,
        q: &Q,
        _env: &AbstractEnv<Self::Domain>,
        _state: &StateStore<Self::Domain>,
        _memo: &MemoStore<Self::Domain>,
        ctx: &dyn QueryContext,
    ) -> Q::Result {
        // Project to T1's domain via bottom env (conservative but type-correct).
        // Full projection requires map_values on stores TODO when fused fixpoint
        // is wired in. For now, T1 can use its own query logic with empty context.
        let empty_env = AbstractEnv::bottom();
        let empty_state = StateStore::bottom();
        let empty_memo = MemoStore::new();
        self.0.ask(q, &empty_env, &empty_state, &empty_memo, ctx)
    }
}

impl<T1: Transfer, T2: Transfer> Transfer for ProductTransfer<T1, T2>
where
    ProductDomain<T1::Domain, T2::Domain>: AbstractDomain,
{
    type Domain = ProductDomain<T1::Domain, T2::Domain>;

    fn eval_expr(
        &self,
        _expr: &crate::ir::expr::Expr,
        _env: &AbstractEnv<Self::Domain>,
        _ctx: &mut AnalysisCtx<Self::Domain>,
    ) -> Self::Domain {
        // TODO: implement full projection/injection when fused fixpoint is needed.
        // For now ProductTransfer exists for Queryable delegation only.
        ProductDomain::bottom()
    }

    fn exec_stmt(
        &self,
        _stmt: &crate::ir::stmt::Stmt,
        _env: &mut AbstractEnv<Self::Domain>,
        _ctx: &mut AnalysisCtx<Self::Domain>,
    ) {
        // TODO: implement full projection/injection when fused fixpoint is needed.
    }

    fn recompute_memo(
        &self,
        _component: &crate::ir::types::Symbol,
        _deps: &[crate::ir::expr::Expr],
        _env: &AbstractEnv<Self::Domain>,
        _ctx: &dyn QueryContext,
    ) -> Self::Domain {
        ProductDomain::bottom()
    }
}
