//! The rules layer (ADR-006): pure post-passes over the converged fixpoint.
//!
//! Layout:
//! - [`api`] — the typed surface a rule programs against (ADR-021): query
//!   primitives, the sealed [`Diagnostic`], the witness vocabulary.
//! - [`impls`] — the rule implementations, one file per rule.
//! - [`helpers`] — shared analysis machinery (setter/churn/eval/scans).
//! - [`docs`] — static documentation for every diagnostic name.
//! - [`declarative`] — the declarative pack frontend (ADR-022 Tier A):
//!   loader, validator, executor.
//!
//! This module keeps only the [`Rule`] trait, [`SafeCheck`], the native rule
//! set ([`all_rules`]), the dynamic [`registry`] (ADR-022) and the public
//! façade — every name re-exported here is at its historical path, so
//! consumers never reach into submodules.

pub mod api;
pub mod declarative;
pub mod docs;
pub mod helpers;
pub mod impls;
pub mod registry;

pub use api::diagnostic::{Diagnostic, Severity};
pub use api::query::{
    Certified, ConditionalHookCall, DominatesAllExits, EffectCycleProof, ExitDominance,
    InitSetterCall, May, Motion, MovingFeeder, MustResult, OnAllPaths, Provenance, RuleConfig,
    RuleCtx, SameRefMutation, StabilityVerdict, classify_motion, may_change_of,
    must_dominates_all_exits, must_frozen_seed, must_init_calls_setter, must_on_all_paths,
    must_same_ref_mutation, must_setter_on_all_paths, stability_verdict_of,
};
pub use api::witness::{EffectClass, Note, ResolveTarget, Step, ValueClass};
pub use docs::{RULE_DOCS, RuleDoc, rule_doc};
pub use registry::{
    ComponentFindings, OverrideEntry, RegistryError, RuleOverrides, RuleRegistry,
};
pub use helpers::setters::{SetterCall, collect_setter_calls, collect_setter_calls_with_extra};
pub use impls::{
    AlwaysUnstableDeps, AnalysisLimitInfo, ConditionalHook, DerivedState, FrozenInitialState,
    InfiniteLoop, LazyInit, MissingDeps, RedundantSetState, SetterInRender, StaleClosure,
    StateMutation, UnnecessaryRerender, WideningInfo,
};

// Internal vocabulary shared by api/helpers/impls, re-exported at its
// historical paths so call sites stay `crate::rules::X`.
pub(crate) use helpers::setters::{
    all_setter_labels, memo_val_labels, resolve_setter_aliases, setter_var_labels, state_val_labels,
};
pub(in crate::rules) use helpers::setters::{
    collect_fn_bindings, cross_component_setters, may_written_slots,
};
pub(crate) use helpers::{
    ConvergedEval, arg_is_call_free, collect_callees, describe_value, eval_in_stores,
    has_hook_kind, hook_kind_word, local_bindings, state_slot_name,
};
pub(in crate::rules) use helpers::{all_deps_provably_stable, fn_lit_binding};

/// A check that was *applicable* to a component and found nothing wrong —
/// surfaced under `--info` as positive assurance ("verified: …").
///
/// Distinct from an absent diagnostic: emptiness alone cannot tell "the
/// infinite-loop check ran and the component is safe" from "there was no
/// useState/useEffect for it to check". A `SafeCheck` records the former only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeCheck {
    /// Diagnostic name the assurance corresponds to (matches `RuleDoc::name`).
    pub rule: &'static str,
    /// Present-tense assurance, e.g. "no effect diverges into an infinite loop".
    pub message: &'static str,
}

/// Post-pass analysis rule operating on a fully-computed `AnalysisResult`.
///
/// Rules are stateless; adding a new rule = new struct + `impl Rule`.
///
/// Both methods bind to [`RuleCtx`] (ADR-021 §4): the caller resolves the
/// component once (`RuleCtx::new`), and the ctx is the stable anchor the
/// future external frontends bind to.
pub trait Rule {
    /// Rule id. Borrowed from `self` so dynamically loaded rules (ADR-022) can
    /// own their `pack/rule` id; native impls keep returning `&'static str`.
    fn name(&self) -> &str;
    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic>;

    /// When this rule is *applicable* to the ctx's component but `check` found
    /// nothing, the positive assurance to surface under `--info`.
    ///
    /// Only consulted after `check` returned no diagnostics for the component,
    /// so implementations decide *applicability* only — they need not re-check.
    /// Default `None`: the rule opts out (e.g. Info-limitation rules, which have
    /// no "safe" state to report).
    fn safe_check(&self, _ctx: &RuleCtx) -> Option<SafeCheck> {
        None
    }
}

/// Instantiate all built-in rules.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(ConditionalHook),
        Box::new(MissingDeps),
        Box::new(AlwaysUnstableDeps),
        Box::new(LazyInit),
        Box::new(RedundantSetState),
        Box::new(UnnecessaryRerender),
        Box::new(SetterInRender),
        Box::new(StaleClosure),
        Box::new(StateMutation),
        Box::new(InfiniteLoop),
        Box::new(DerivedState),
        Box::new(FrozenInitialState),
        Box::new(WideningInfo),
        Box::new(AnalysisLimitInfo),
    ]
}
