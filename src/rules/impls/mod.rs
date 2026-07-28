//! The rule implementations — one file per rule, nothing else (ADR-006: pure
//! post-passes over the converged fixpoint). Shared machinery lives in
//! [`crate::rules::helpers`]; the typed surface they consume lives in
//! [`crate::rules::api`].

pub mod always_unstable_deps;
pub mod analysis_limit_info;
pub mod conditional_hook;
pub mod derived_state;
pub mod frozen_initial_state;
pub mod infinite_loop;
pub mod lazy_init;
pub mod missing_cleanup;
pub mod missing_deps;
pub mod redundant_set_state;
pub mod setter_in_render;
pub mod stale_closure;
pub mod state_mutation;
pub mod unnecessary_rerender;
pub mod unstable_context_value;
pub mod widening_info;

pub use always_unstable_deps::AlwaysUnstableDeps;
pub use analysis_limit_info::AnalysisLimitInfo;
pub use conditional_hook::ConditionalHook;
pub use derived_state::DerivedState;
pub use frozen_initial_state::FrozenInitialState;
pub use infinite_loop::InfiniteLoop;
pub use lazy_init::LazyInit;
pub use missing_cleanup::MissingCleanup;
pub use missing_deps::MissingDeps;
pub use redundant_set_state::RedundantSetState;
pub use setter_in_render::SetterInRender;
pub use stale_closure::StaleClosure;
pub use state_mutation::StateMutation;
pub use unnecessary_rerender::UnnecessaryRerender;
pub use unstable_context_value::UnstableContextValue;
pub use widening_info::WideningInfo;
