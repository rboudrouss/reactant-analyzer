//! The Tier-A executor: a validated [`ResolvedRule`] running as a [`Rule`].
//!
//! Severity is `pin ⊓ polarity`, per finding, at emission (ADR-022 §3): the
//! finding's ceiling is Error iff a must-guard certified *this* finding
//! (a `Certified` proof is held), Warning otherwise. The clamp is
//! structural, not policed — this module sits under `src/rules/` like every
//! native rule, so it cannot mint a `Certified` (private to `api::query`)
//! and cannot construct an Error except through `Diagnostic::error(proof, …)`.
//! Provenance rides the proof, so `--trace` works on custom findings with
//! zero author effort (§8). Custom rules have no `safe_check` in v1 (the
//! trait default).

use std::collections::HashSet;

use crate::ir::SourceRange;
use crate::ir::hooks::{Arity, HookEntry};
use crate::ir::types::HookLabel;
use crate::rules::api::diagnostic::Diagnostic;
use crate::rules::api::query::{
    Certified, ConditionalHookCall, DirectWrite, DominatesAllExits, InitSetterCall, MustResult,
    Provenance, RuleCtx, must_direct_write, must_init_calls_setter, must_setter_on_all_paths,
};
use crate::rules::{Rule, SetterCall};

use super::entity::{
    ArgEntity, DepEntity, EntityCtx, EntityVal, HookRow, SetterEntity, cleanup_name, firing_name,
    identity_name, phase_name, provider_name, seed_sync_name, teardown_name, updater_name,
};
use super::schema::{EdgeName, ElseBehavior, OwnershipName, SeverityPin};
use super::validate::{
    BindRef, CountCmp, MustKind, ResolvedAnchor, ResolvedGuard, ResolvedRule, Segment,
};
use crate::rules::helpers::churn_graph::CycleRow;
use crate::rules::helpers::jsx::JsxPropSite;
use crate::rules::helpers::providers::ProviderSite;

pub(crate) struct TierARule {
    pub def: ResolvedRule,
}

/// A held certification for the finding under evaluation. The enum exists
/// because different must-guards certify different evidence types; emission
/// matches to reach the generic `Diagnostic::error`.
enum Proof {
    Setter(Certified<SetterCall>),
    Dominates(Certified<DominatesAllExits>),
    Init(Certified<InitSetterCall>),
    Conditional(Certified<ConditionalHookCall>),
    Direct(Certified<DirectWrite>),
}

impl Proof {
    fn provenance(&self) -> &Provenance {
        match self {
            Proof::Setter(c) => c.provenance(),
            Proof::Dominates(c) => c.provenance(),
            Proof::Init(c) => c.provenance(),
            Proof::Conditional(c) => c.provenance(),
            Proof::Direct(c) => c.provenance(),
        }
    }
}

/// The `forEach` binding's value for one finding.
enum Bound<'a, 'b> {
    Setter(&'b SetterEntity),
    Dep(&'b DepEntity<'a>),
    Arg(&'b ArgEntity),
    Writer(&'a crate::engine::SlotWriter),
    Seed(&'a crate::engine::SlotSeed),
    /// One non-hook call site in the anchor's body (#126).
    Call(&'b crate::engine::BodyCall),
    /// One read site of the anchor slot (#127).
    Read(&'b crate::engine::SlotRead),
}

/// One candidate under evaluation: whatever the anchor bound, plus the
/// `forEach` element if the rule navigates an edge. The two anchors used to
/// have a guard match each, so a guard could be handled on one and silently
/// fall into an `unreachable!` catch-all on the other.
enum Candidate<'a, 'b> {
    Hook {
        row: &'b HookRow<'a>,
        bound: Option<Bound<'a, 'b>>,
    },
    RenderSetter(&'b SetterEntity),
    /// One `hook_origins` row (ADR-027 §7) — edge-less by construction.
    Origin(&'a crate::ir::hooks::HookProvenance),
    /// One `context_providers` row (#71) — edge-less in v1.
    Provider(&'b ProviderSite<'a>),
    JsxProp(&'b JsxPropSite<'a>),
    /// One render-loop cycle carried by this component's effects (#108) —
    /// edge-less, and no must-guard accepts its sort, so it can never mint a
    /// proof.
    Cycle(&'b CycleRow),
    /// One `useContext` call site with complete ancestry (#115) — edge-less,
    /// and no must-guard accepts its sort.
    Consumer(&'a crate::rules::helpers::context_flow::ConsumerRow),
    /// One callback registration in an effect body (#111) — edge-less, and no
    /// must-guard accepts its sort: the relation is a may-registration.
    Registration(&'a crate::engine::registrations::Registration),
    /// One non-hook call site in the render body (#126) — edge-less, and no
    /// must-guard accepts its sort: the callee is a resolved binding, never a
    /// proof of which host primitive runs.
    RenderCall(&'b crate::engine::BodyCall),
    /// One element the render body builds (#126), optionally navigated to one
    /// of its props. No must-guard accepts either sort.
    Element {
        site: &'b crate::rules::helpers::jsx::JsxElementSite<'a>,
        prop: Option<&'b JsxPropSite<'a>>,
    },
}

impl<'a, 'b> Candidate<'a, 'b> {
    fn row(&self) -> Option<&'b HookRow<'a>> {
        match self {
            Candidate::Hook { row, .. } => Some(row),
            Candidate::RenderSetter(_)
            | Candidate::Origin(_)
            | Candidate::Provider(_)
            | Candidate::JsxProp(_)
            | Candidate::Cycle(_)
            | Candidate::Consumer(_)
            | Candidate::Registration(_)
            | Candidate::RenderCall(_)
            | Candidate::Element { .. } => None,
        }
    }

    fn bound(&self) -> Option<&Bound<'a, 'b>> {
        match self {
            Candidate::Hook { bound, .. } => bound.as_ref(),
            Candidate::RenderSetter(_)
            | Candidate::Origin(_)
            | Candidate::Provider(_)
            | Candidate::JsxProp(_)
            | Candidate::Cycle(_)
            | Candidate::Consumer(_)
            | Candidate::Registration(_)
            | Candidate::RenderCall(_)
            | Candidate::Element { .. } => None,
        }
    }

    /// Resolve a guard/template subject to its entity value.
    fn entity_at(&self, r: BindRef) -> EntityVal<'a, '_> {
        match (r, self) {
            (BindRef::Anchor, Candidate::Hook { row, .. }) => EntityVal::Hook(row),
            (BindRef::Anchor, Candidate::RenderSetter(s)) => EntityVal::Setter(s),
            (BindRef::Anchor, Candidate::Origin(p)) => EntityVal::Origin(p),
            (BindRef::Anchor, Candidate::Provider(p)) => EntityVal::Provider(p),
            (BindRef::Anchor, Candidate::JsxProp(j)) => EntityVal::JsxProp(j),
            (BindRef::Anchor, Candidate::Cycle(c)) => EntityVal::Cycle(c),
            (BindRef::Anchor, Candidate::Registration(r)) => EntityVal::Registration(r),
            (BindRef::Anchor, Candidate::Consumer(c)) => EntityVal::Consumer(c),
            (BindRef::Anchor, Candidate::RenderCall(c)) => EntityVal::Call(c),
            (BindRef::Anchor, Candidate::Element { site, .. }) => EntityVal::Element(site),
            (BindRef::Bound, Candidate::Element { prop, .. }) => {
                EntityVal::JsxProp(prop.expect("validated: binding exists"))
            }
            (BindRef::Bound, _) => match self.bound().expect("validated: binding exists") {
                Bound::Setter(s) => EntityVal::Setter(s),
                Bound::Dep(d) => EntityVal::Dep(d),
                Bound::Arg(a) => EntityVal::Arg(a),
                Bound::Writer(w) => EntityVal::Writer(w),
                Bound::Seed(s) => EntityVal::Seed(s),
                Bound::Call(c) => EntityVal::Call(c),
                Bound::Read(r) => EntityVal::Read(r),
            },
        }
    }

    /// The hook label a finding on this candidate carries.
    fn label(&self) -> Option<HookLabel> {
        match self {
            Candidate::Hook { row, .. } => Some(row.info.label),
            Candidate::RenderSetter(s) => s.slot,
            Candidate::Origin(p) => Some(p.label),
            Candidate::Provider(_)
            | Candidate::JsxProp(_)
            | Candidate::RenderCall(_)
            | Candidate::Element { .. } => None,
            // The carrying effect: the finding is about that effect's write.
            Candidate::Cycle(c) => Some(c.effect),
            Candidate::Consumer(c) => Some(c.label),
            // The registering effect: the finding is about that effect.
            Candidate::Registration(r) => Some(r.effect),
        }
    }

    /// Where the finding is anchored: the bound setter's call site when the
    /// rule navigated to one, the hook call otherwise.
    fn range(&self) -> Option<SourceRange> {
        match self {
            Candidate::Hook { row, bound } => match bound {
                Some(Bound::Setter(s)) => s.span,
                Some(Bound::Writer(w)) => w.span.or(row.info.span),
                // A call row's own site, not the hook's: the whole point of
                // the relation is to point at the call.
                Some(Bound::Call(c)) => c.span.or(row.info.span),
                Some(Bound::Read(r)) => r.span.or(row.info.span),
                Some(Bound::Dep(_) | Bound::Arg(_) | Bound::Seed(_)) | None => row.info.span,
            },
            Candidate::RenderSetter(s) => s.span,
            // The provenance row's own call-site span: the label can dangle
            // (an expanded wrapper keeps its direct row but loses its entry),
            // so there is no `hook_calls` row to borrow a range from.
            Candidate::Origin(p) => p.span,
            Candidate::Provider(p) => p.span,
            Candidate::JsxProp(j) => j.span,
            // ADR-024: the carrying edge's write site, which is why a spanless
            // edge produces no row at all.
            Candidate::Cycle(c) => Some(c.span),
            Candidate::Consumer(c) => c.span,
            Candidate::Registration(r) => r.span,
            Candidate::RenderCall(c) => c.span,
            Candidate::Element { site, prop } => prop.and_then(|p| p.span).or(site.span),
        }
    }
}

impl Rule for TierARule {
    fn name(&self) -> &str {
        &self.def.id
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let e = EntityCtx::new(ctx);
        let mut out = Vec::new();
        match self.def.anchor {
            ResolvedAnchor::HookCalls(kind) => {
                for row in e.hook_rows(kind) {
                    match self.def.for_each {
                        Some(EdgeName::Deps) => {
                            for dep in e.deps(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Dep(&dep)),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        Some(EdgeName::BodySetterCalls) => {
                            for setter in e.body_setters(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Setter(&setter)),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        Some(EdgeName::Calls) => {
                            for call in e.body_calls(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Call(call)),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        Some(EdgeName::Args) => {
                            for arg in e.args(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Arg(&arg)),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        // Validated: `props` needs an `elements` anchor.
                        Some(EdgeName::Props) => {}
                        Some(EdgeName::Reads) => {
                            for read in e.reads(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Read(read)),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        Some(EdgeName::Seeds) => {
                            for seed in e.seeds(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Seed(seed)),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        Some(EdgeName::Writers) => {
                            for writer in e.writers(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Writer(writer)),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        None => self.eval(
                            &e,
                            &Candidate::Hook {
                                row: &row,
                                bound: None,
                            },
                            &mut out,
                        ),
                    }
                }
            }
            ResolvedAnchor::RenderSetterCalls { foreign } => {
                for setter in e.render_setters(foreign) {
                    self.eval(&e, &Candidate::RenderSetter(&setter), &mut out);
                }
            }
            // Edge-less (validated): no forEach dispatch to do.
            ResolvedAnchor::HookOrigins => {
                for p in e.origin_rows() {
                    self.eval(&e, &Candidate::Origin(p), &mut out);
                }
            }
            ResolvedAnchor::ContextProviders => {
                for site in e.provider_rows() {
                    self.eval(&e, &Candidate::Provider(&site), &mut out);
                }
            }
            ResolvedAnchor::RenderCalls => {
                for call in e.render_call_rows() {
                    self.eval(&e, &Candidate::RenderCall(&call), &mut out);
                }
            }
            ResolvedAnchor::Elements { elements } => {
                for site in e.jsx_element_rows(elements) {
                    match self.def.for_each {
                        Some(EdgeName::Props) => {
                            for prop in &site.props {
                                self.eval(
                                    &e,
                                    &Candidate::Element {
                                        site: &site,
                                        prop: Some(prop),
                                    },
                                    &mut out,
                                );
                            }
                        }
                        _ => self.eval(
                            &e,
                            &Candidate::Element {
                                site: &site,
                                prop: None,
                            },
                            &mut out,
                        ),
                    }
                }
            }
            ResolvedAnchor::JsxProps { elements } => {
                for site in e.jsx_prop_rows(elements) {
                    self.eval(&e, &Candidate::JsxProp(&site), &mut out);
                }
            }
            ResolvedAnchor::ChurnCycles => {
                for row in e.cycle_rows() {
                    self.eval(&e, &Candidate::Cycle(&row), &mut out);
                }
            }
            ResolvedAnchor::ContextConsumers => {
                for row in e.consumer_rows() {
                    self.eval(&e, &Candidate::Consumer(row), &mut out);
                }
            }
            ResolvedAnchor::Registrations { firing } => {
                for row in e
                    .registration_rows()
                    .iter()
                    .filter(|r| firing.is_none_or(|f| f == firing_name(r.firing)))
                {
                    self.eval(&e, &Candidate::Registration(row), &mut out);
                }
            }
        }
        out
    }
}

impl TierARule {
    /// Evaluate every guard against one candidate; emit on a full pass. Guards
    /// run in author order and short-circuit on the first failure.
    fn eval(&self, e: &EntityCtx<'_>, cand: &Candidate<'_, '_>, out: &mut Vec<Diagnostic>) {
        let mut proofs: Vec<Proof> = Vec::new();
        for guard in &self.def.guards {
            if !self.eval_guard(e, cand, guard, &mut proofs) {
                return;
            }
        }

        let message: String = self
            .def
            .message
            .iter()
            .map(|seg| match seg {
                Segment::Lit(s) => s.clone(),
                Segment::Field(r, f) => e.render_field(&cand.entity_at(*r), *f),
            })
            .collect();
        out.push(self.emit(message, proofs, cand.label(), cand.range()));
    }

    /// One guard against one candidate. Recursive for `any_of`; proofs
    /// collected along the way are pushed onto `proofs` whether or not the
    /// guard ends up passing, because a certified sub-claim is evidence for
    /// the finding either way.
    fn eval_guard(
        &self,
        e: &EntityCtx<'_>,
        cand: &Candidate<'_, '_>,
        guard: &ResolvedGuard,
        proofs: &mut Vec<Proof>,
    ) -> bool {
        // Every `bound` match below is exhaustive, not a refutable `let`: a
        // new edge would otherwise validate, load, and silently emit nothing.
        match guard {
            ResolvedGuard::Stability { names, negated, .. } => match cand.bound() {
                Some(Bound::Dep(dep)) => names.contains(&e.dep_verdict(dep)) != *negated,
                Some(
                    Bound::Setter(_)
                    | Bound::Arg(_)
                    | Bound::Writer(_)
                    | Bound::Seed(_)
                    | Bound::Call(_)
                    | Bound::Read(_),
                )
                | None => {
                    unreachable!("validated: `stability` binds a deps entry")
                }
            },
            ResolvedGuard::Returns { names, negated, .. } => match cand.bound() {
                Some(Bound::Arg(arg)) => names.contains(&e.arg_verdict(arg)) != *negated,
                Some(
                    Bound::Setter(_)
                    | Bound::Dep(_)
                    | Bound::Writer(_)
                    | Bound::Seed(_)
                    | Bound::Call(_)
                    | Bound::Read(_),
                )
                | None => {
                    unreachable!("validated: `returns` binds a call-site argument")
                }
            },
            ResolvedGuard::Origin { hook, direct, .. } => {
                // Validated: the subject is a hook-call or hook-origin row,
                // which only the anchor can bind in v1. Positive-only: no
                // provenance row ⇒ fail.
                let prov = match cand {
                    Candidate::Origin(p) => Some(*p),
                    _ => {
                        let Some(row) = cand.row() else {
                            unreachable!("validated: `origin` binds a hook-call row")
                        };
                        e.provenance(row.info.label)
                    }
                };
                match prov {
                    Some(p) => {
                        hook.as_ref()
                            .is_none_or(|names| names.iter().any(|n| n == p.origin_hook.as_str()))
                            && direct.is_none_or(|d| p.inlined != d)
                    }
                    None => false,
                }
            }
            ResolvedGuard::InDeps { negate, .. } => match (cand.row(), cand.bound()) {
                (Some(row), Some(Bound::Setter(setter))) => {
                    let in_deps = setter
                        .slot
                        .is_some_and(|slot| e.dep_slots(row).contains(&slot));
                    in_deps != *negate
                }
                _ => unreachable!("validated: `in_deps` binds a body setter call"),
            },
            ResolvedGuard::Text {
                of,
                field,
                one_of,
                prefix,
            } => text_matches(e.field_raw(&cand.entity_at(*of), *field), one_of, prefix),
            ResolvedGuard::Identity { of, names, negated } => {
                // Both JSX relations carry the one shared verdict.
                let identity = match cand.entity_at(*of) {
                    EntityVal::Provider(p) => p.identity,
                    EntityVal::JsxProp(j) => j.identity,
                    EntityVal::Arg(a) => e.arg_identity(a),
                    EntityVal::Registration(r) => e.listener_identity(r),
                    _ => unreachable!(
                        "validated: `identity` binds a JSX site, an argument or a registration"
                    ),
                };
                names.contains(&identity_name(identity)) != *negated
            }
            ResolvedGuard::Cleanup { of, names, negated } => {
                // Validated: `of` binds a kind-pinned effect anchor, so the
                // verdict is a property of the anchor's OWN body CFG — read at
                // the anchor's position, not at some later program point
                // (ADR-023 §2 is not in play).
                let _ = of;
                let Some(row) = cand.row() else {
                    unreachable!("validated: `cleanup` reads an effect-hook anchor")
                };
                names.contains(&cleanup_name(e.cleanup(row))) != *negated
            }
            ResolvedGuard::Updater { of, names } => {
                let EntityVal::Writer(w) = cand.entity_at(*of) else {
                    unreachable!("validated: `updater` binds a writers row")
                };
                names.contains(&updater_name(&w.updater))
            }
            ResolvedGuard::UpdaterBody { of, names } => {
                let EntityVal::Writer(w) = cand.entity_at(*of) else {
                    unreachable!("validated: `updater_body` binds a writers row")
                };
                names.contains(&e.updater_purity(&w.updater))
            }
            ResolvedGuard::Phase { of, names } => {
                let phase = match cand.entity_at(*of) {
                    EntityVal::Call(c) => c.phase,
                    EntityVal::Read(r) => r.phase,
                    _ => unreachable!("validated: `phase` binds a calls or reads row"),
                };
                names.contains(&phase_name(phase))
            }
            ResolvedGuard::Provider { of, names } => {
                let EntityVal::Consumer(c) = cand.entity_at(*of) else {
                    unreachable!("validated: `provider` binds a context_consumers row")
                };
                names.contains(&provider_name(c.verdict))
            }
            ResolvedGuard::SeedSync { of, names } => {
                let EntityVal::Seed(s) = cand.entity_at(*of) else {
                    unreachable!("validated: `seed_sync` binds a seeds row")
                };
                names.contains(&seed_sync_name(s.sync))
            }
            ResolvedGuard::Teardown { of, names } => {
                let EntityVal::Registration(r) = cand.entity_at(*of) else {
                    unreachable!("validated: `teardown` binds a registrations row")
                };
                names.contains(&teardown_name(r.pairing))
            }
            ResolvedGuard::Registers { of, firing } => {
                let EntityVal::Hook(h) = cand.entity_at(*of) else {
                    unreachable!("validated: `registers` binds an effect anchor")
                };
                e.registration_rows()
                    .iter()
                    .filter(|r| r.effect == h.info.label)
                    .any(|r| firing.contains(&firing_name(r.firing)))
            }
            ResolvedGuard::SlotOwnership { of, names } => {
                let EntityVal::Setter(s) = cand.entity_at(*of) else {
                    unreachable!("validated: `slot_ownership` binds a render setter call")
                };
                let owned = if s.owner.is_some() {
                    OwnershipName::Foreign
                } else {
                    OwnershipName::Local
                };
                names.contains(&owned)
            }
            ResolvedGuard::Cycle {
                of,
                cross_component,
                all_must,
            } => {
                let EntityVal::Cycle(c) = cand.entity_at(*of) else {
                    unreachable!("validated: `cycle` binds a churn_cycles row")
                };
                // Conjunction of the given fields; both are exact folds of the
                // graph, so the negative is as answerable as the positive.
                cross_component.is_none_or(|x| c.cross_component == x)
                    && all_must.is_none_or(|x| c.all_must == x)
            }
            ResolvedGuard::SameTick { of } => {
                let EntityVal::Writer(w) = cand.entity_at(*of) else {
                    unreachable!("validated: `same_tick` binds a writers row")
                };
                w.same_tick
            }
            ResolvedGuard::Provenance {
                of,
                through,
                direct,
            } => {
                let EntityVal::Writer(w) = cand.entity_at(*of) else {
                    unreachable!("validated: `provenance` binds a writers row")
                };
                use crate::engine::setters::WriteProvenance;
                // Conjunction of the given fields; an unplaceable site fails
                // both forms (positive-only, ADR-027 §4).
                match &w.via {
                    WriteProvenance::Unknown => false,
                    WriteProvenance::Direct => through.is_none() && direct.is_none_or(|d| d),
                    WriteProvenance::Via(chain) => {
                        through
                            .as_ref()
                            .is_none_or(|names| chain.iter().any(|c| names.iter().any(|n| n == c)))
                            && direct.is_none_or(|d| !d)
                    }
                }
            }
            ResolvedGuard::WriterPhases { includes } => {
                // Validated: the anchor is a state hook — its label is the
                // slot. MAY existential: a ⊤-phase write satisfies any query.
                let Some(row) = cand.row() else {
                    unreachable!("validated: `writer_phases` reads a state-hook anchor")
                };
                e.writer_phase_includes(row.info.label, includes)
            }
            // An arity guard needs an arity. A hook with no readable deps
            // array — or one whose lowering dropped or flattened an element —
            // does not have zero dependencies, it has an unknown number of
            // them, so the guard refuses rather than answering from a length
            // that is not the source array's.
            // An arity guard needs a written array to count. There is none
            // when the caller passed no deps argument, or one the engine
            // cannot read — that is knowledge, not ignorance, and
            // `deps_declared` is the guard that asks about it.
            ResolvedGuard::Count(cmp) => {
                let Some(arity) = cand
                    .row()
                    .and_then(|r| r.effect)
                    .and_then(|i| i.deps.list())
                    .map(|l| l.arity)
                else {
                    return false;
                };
                match arity {
                    Arity::Exact(m) => {
                        let m = m as u64;
                        match cmp {
                            CountCmp::MoreThan(n) => m > *n,
                            CountCmp::LessThan(n) => m < *n,
                            CountCmp::Equals(n) => m == *n,
                        }
                    }
                    // A flattened spread leaves only a lower bound, so the
                    // guard answers what that bound *refutes* and passes
                    // otherwise. Refusing outright deleted findings: the arity
                    // of `[a, …, g, ...rest]` provably exceeds any budget its
                    // visible elements already exceed.
                    Arity::AtLeast(m) => {
                        let m = m as u64;
                        match cmp {
                            CountCmp::MoreThan(_) => true,
                            CountCmp::LessThan(n) => m < *n,
                            CountCmp::Equals(n) => m <= *n,
                        }
                    }
                }
            }
            ResolvedGuard::DepsDeclared { eq } => {
                cand.row()
                    .and_then(|r| r.effect)
                    .is_some_and(|i| i.has_deps_array())
                    == *eq
            }
            ResolvedGuard::Must { kind, els, .. } => match (self.certify(e, cand, *kind), els) {
                (Some(p), _) => {
                    proofs.push(p);
                    true
                }
                (None, ElseBehavior::Keep) => true,
                (None, ElseBehavior::Drop) => false,
            },
            // Every branch is evaluated: short-circuiting would make whether a
            // `must_*` branch contributes its proof — and therefore whether the
            // finding can reach Error — depend on the order the author wrote
            // the branches in.
            ResolvedGuard::AnyOf(children) => {
                // Not `.any()`: it short-circuits, and each call pushes proofs.
                let mut passed = false;
                for child in children {
                    passed |= self.eval_guard(e, cand, child, proofs);
                }
                passed
            }
            // `none` over an element's props: the anchor is not a hook row, so
            // this is its own arm rather than a case of the one below.
            ResolvedGuard::NoneOf {
                edge: EdgeName::Props,
                body,
            } => {
                let Candidate::Element { site, .. } = cand else {
                    unreachable!("validated: `props` binds an `elements` anchor")
                };
                let mut scratch: Vec<Proof> = Vec::new();
                !site.props.iter().any(|prop| {
                    let elem = Candidate::Element {
                        site,
                        prop: Some(prop),
                    };
                    body.iter()
                        .all(|g| self.eval_guard(e, &elem, g, &mut scratch))
                })
            }
            ResolvedGuard::NoneOf { edge, body } => {
                let Some(row) = cand.row() else {
                    unreachable!("validated: `none` quantifies over an anchor's edge")
                };
                // Same second lock as `every`: the validator refuses a
                // `must_*` inside, so this scratch vector stays empty and no
                // Error authority can leak out of a negated existential.
                let mut scratch: Vec<Proof> = Vec::new();
                let mut hit = |bound: Bound<'_, '_>| {
                    let elem = Candidate::Hook {
                        row,
                        bound: Some(bound),
                    };
                    body.iter()
                        .all(|g| self.eval_guard(e, &elem, g, &mut scratch))
                };
                let any = match edge {
                    EdgeName::Deps => e.deps(row).iter().any(|d| hit(Bound::Dep(d))),
                    EdgeName::BodySetterCalls => {
                        e.body_setters(row).iter().any(|s| hit(Bound::Setter(s)))
                    }
                    EdgeName::Args => e.args(row).iter().any(|a| hit(Bound::Arg(a))),
                    EdgeName::Writers => e.writers(row).iter().any(|w| hit(Bound::Writer(w))),
                    EdgeName::Seeds => e.seeds(row).iter().any(|s| hit(Bound::Seed(s))),
                    EdgeName::Calls => e.body_calls(row).iter().any(|c| hit(Bound::Call(c))),
                    EdgeName::Reads => e.reads(row).iter().any(|r| hit(Bound::Read(r))),
                    EdgeName::Props => unreachable!("handled above"),
                };
                !any
            }
            ResolvedGuard::Every(body) => {
                let Some(row) = cand.row() else {
                    unreachable!("validated: `every` quantifies over a deps-bearing anchor")
                };
                // Quantifying needs a domain. A written array supplies one
                // even when a spread hides part of it — the fold then ranges
                // over the elements the engine can see and refutes ∀ as soon
                // as one of them violates. An absent or unreadable argument
                // supplies no element at all, and a claim about nothing is not
                // a claim the engine may make.
                if row.effect.and_then(|i| i.deps.list()).is_none() {
                    return false;
                }
                // `proofs` is deliberately not threaded in: a may-typed
                // quantifier must never contribute Error authority. The
                // validator already refuses a `must_*` inside the body, so
                // this scratch vector stays empty — it is the second lock.
                let mut scratch: Vec<Proof> = Vec::new();
                let all = e.deps(row).into_iter().all(|dep| {
                    let elem = Candidate::Hook {
                        row,
                        bound: Some(Bound::Dep(&dep)),
                    };
                    body.iter()
                        .all(|g| self.eval_guard(e, &elem, g, &mut scratch))
                });
                debug_assert!(scratch.is_empty(), "`every` must not mint a proof");
                all
            }
        }
    }

    /// Run the must-primitive backing `kind` for this finding's subject.
    fn certify(
        &self,
        e: &EntityCtx<'_>,
        cand: &Candidate<'_, '_>,
        kind: MustKind,
    ) -> Option<Proof> {
        match kind {
            MustKind::SetterOnAllPaths => {
                let (row, setter) = match (cand.row(), cand.bound()) {
                    (Some(row), Some(Bound::Setter(s))) => (row, s),
                    _ => unreachable!("validated: `must_setter_on_all_paths` binds a body setter"),
                };
                let body = row.entry.and_then(|en| en.body_cfg())?;
                // The alias set for the subject's slot — the primitive's own
                // must-forwarding handles multi-site/branchy writes, which
                // the deduplicated `SetterCall.block_id` could not.
                let slot = setter.slot?;
                let aliases: HashSet<_> = e
                    .setter_labels
                    .iter()
                    .filter(|(_, l)| **l == slot)
                    .map(|(v, _)| v.clone())
                    .collect();
                match must_setter_on_all_paths(body, &aliases, None) {
                    MustResult::All(c) => Some(Proof::Setter(c)),
                    _ => None,
                }
            }
            MustKind::DominatesAllExits => {
                let setter = match cand {
                    Candidate::RenderSetter(s) => s,
                    Candidate::Hook { .. }
                    | Candidate::Origin(_)
                    | Candidate::Provider(_)
                    | Candidate::JsxProp(_)
                    | Candidate::Cycle(_)
                    | Candidate::Consumer(_)
                    | Candidate::Registration(_)
                    | Candidate::RenderCall(_)
                    | Candidate::Element { .. } => {
                        unreachable!("validated: `must_dominates_all_exits` binds a render setter")
                    }
                };
                setter.block_id.and_then(|b| match e.exit_dom().certify(b) {
                    MustResult::All(c) => Some(Proof::Dominates(c)),
                    _ => None,
                })
            }
            MustKind::InitCallsSetter => {
                let row = cand.row().expect("validated: hook anchor");
                let init = match row.entry {
                    Some(HookEntry::State { init, .. } | HookEntry::Ref { init, .. }) => init,
                    _ => return None,
                };
                match must_init_calls_setter(init, &e.setter_vars) {
                    MustResult::All(c) => Some(Proof::Init(c)),
                    _ => None,
                }
            }
            MustKind::HookIsConditional => {
                let row = cand.row().expect("validated: hook anchor");
                e.conditional()
                    .get(&row.info.label)
                    .cloned()
                    .map(Proof::Conditional)
            }
            MustKind::DirectWrite => {
                let Some(Bound::Writer(w)) = cand.bound() else {
                    unreachable!("validated: `must_direct_write` binds a writers row")
                };
                match must_direct_write(w) {
                    MustResult::All(c) => Some(Proof::Direct(c)),
                    _ => None,
                }
            }
        }
    }

    /// `effective = pin ⊓ polarity`: Error iff pinned Error AND a proof is
    /// held; the proof's provenance (range/label/notes) rides automatically.
    /// Downgraded/unproven findings still carry the proofs' trace notes.
    fn emit(
        &self,
        message: String,
        mut proofs: Vec<Proof>,
        label: Option<HookLabel>,
        range: Option<SourceRange>,
    ) -> Diagnostic {
        let id = self.def.id.clone();
        let d = match (self.def.pin, proofs.is_empty()) {
            (SeverityPin::Error, false) => {
                let first = proofs.remove(0);
                match first {
                    Proof::Setter(c) => Diagnostic::error(id, c, message),
                    Proof::Dominates(c) => Diagnostic::error(id, c, message),
                    Proof::Init(c) => Diagnostic::error(id, c, message),
                    Proof::Conditional(c) => Diagnostic::error(id, c, message),
                    Proof::Direct(c) => Diagnostic::error(id, c, message),
                }
            }
            (SeverityPin::Error | SeverityPin::Warning, _) => Diagnostic::warn(id, message),
            (SeverityPin::Info, _) => Diagnostic::info(id, message),
        };
        let d = proofs
            .iter()
            .fold(d, |d, p| d.with_notes(p.provenance().notes.clone()));
        let d = match (d.range, range) {
            (None, Some(r)) => d.with_range(r),
            _ => d,
        };
        match (d.hook_label, label) {
            (None, Some(l)) => d.with_label(l),
            _ => d,
        }
    }
}

fn text_matches(
    value: Option<String>,
    one_of: &Option<Vec<String>>,
    prefix: &Option<String>,
) -> bool {
    match (value, one_of, prefix) {
        (Some(n), Some(set), None) => set.iter().any(|s| s == &n),
        (Some(n), None, Some(p)) => n.starts_with(p.as_str()),
        (None, ..) => false,
        _ => unreachable!("validated: exactly one of one_of/prefix"),
    }
}
