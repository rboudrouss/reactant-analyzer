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
use crate::ir::hooks::HookEntry;
use crate::ir::types::HookLabel;
use crate::rules::api::diagnostic::Diagnostic;
use crate::rules::api::query::{
    Certified, ConditionalHookCall, DominatesAllExits, InitSetterCall, MustResult, Provenance,
    RuleCtx, must_init_calls_setter, must_setter_on_all_paths,
};
use crate::rules::{Rule, SetterCall};

use super::entity::{ArgEntity, DepEntity, EntityCtx, EntityVal, HookRow, SetterEntity};
use super::schema::{EdgeName, ElseBehavior, SeverityPin};
use super::validate::{
    BindRef, CountCmp, MustKind, ResolvedAnchor, ResolvedGuard, ResolvedRule, Segment,
};

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
}

impl Proof {
    fn provenance(&self) -> &Provenance {
        match self {
            Proof::Setter(c) => c.provenance(),
            Proof::Dominates(c) => c.provenance(),
            Proof::Init(c) => c.provenance(),
            Proof::Conditional(c) => c.provenance(),
        }
    }
}

/// The `forEach` binding's value for one finding.
enum Bound<'a, 'b> {
    Setter(&'b SetterEntity),
    Dep(&'b DepEntity<'a>),
    Arg(&'b ArgEntity),
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
}

impl<'a, 'b> Candidate<'a, 'b> {
    fn row(&self) -> Option<&'b HookRow<'a>> {
        match self {
            Candidate::Hook { row, .. } => Some(row),
            Candidate::RenderSetter(_) => None,
        }
    }

    fn bound(&self) -> Option<&Bound<'a, 'b>> {
        match self {
            Candidate::Hook { bound, .. } => bound.as_ref(),
            Candidate::RenderSetter(_) => None,
        }
    }

    /// Resolve a guard/template subject to its entity value.
    fn entity_at(&self, r: BindRef) -> EntityVal<'a, '_> {
        match (r, self) {
            (BindRef::Anchor, Candidate::Hook { row, .. }) => EntityVal::Hook(row),
            (BindRef::Anchor, Candidate::RenderSetter(s)) => EntityVal::Setter(s),
            (BindRef::Bound, _) => match self.bound().expect("validated: binding exists") {
                Bound::Setter(s) => EntityVal::Setter(s),
                Bound::Dep(d) => EntityVal::Dep(d),
                Bound::Arg(a) => EntityVal::Arg(a),
            },
        }
    }

    /// The hook label a finding on this candidate carries.
    fn label(&self) -> Option<HookLabel> {
        match self {
            Candidate::Hook { row, .. } => Some(row.info.label),
            Candidate::RenderSetter(s) => s.slot,
        }
    }

    /// Where the finding is anchored: the bound setter's call site when the
    /// rule navigated to one, the hook call otherwise.
    fn range(&self) -> Option<SourceRange> {
        match self {
            Candidate::Hook { row, bound } => match bound {
                Some(Bound::Setter(s)) => s.span,
                Some(Bound::Dep(_) | Bound::Arg(_)) | None => row.info.span,
            },
            Candidate::RenderSetter(s) => s.span,
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
            ResolvedAnchor::RenderSetterCalls => {
                for setter in e.render_setters() {
                    self.eval(&e, &Candidate::RenderSetter(&setter), &mut out);
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
                Some(Bound::Setter(_) | Bound::Arg(_)) | None => {
                    unreachable!("validated: `stability` binds a deps entry")
                }
            },
            ResolvedGuard::Returns { names, negated, .. } => match cand.bound() {
                Some(Bound::Arg(arg)) => names.contains(&e.arg_verdict(arg)) != *negated,
                Some(Bound::Setter(_) | Bound::Dep(_)) | None => {
                    unreachable!("validated: `returns` binds a call-site argument")
                }
            },
            ResolvedGuard::Origin { hook, direct, .. } => {
                // Validated: the subject is a hook-call row, which only the
                // anchor can bind in v1. Positive-only: no provenance row ⇒ fail.
                let Some(row) = cand.row() else {
                    unreachable!("validated: `origin` binds a hook-call row")
                };
                match e.provenance(row.info.label) {
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
            ResolvedGuard::Count(cmp) => {
                let len = cand
                    .row()
                    .and_then(|r| r.effect)
                    .map_or(0, |i| i.declared_deps.len()) as u64;
                match cmp {
                    CountCmp::MoreThan(n) => len > *n,
                    CountCmp::LessThan(n) => len < *n,
                    CountCmp::Equals(n) => len == *n,
                }
            }
            ResolvedGuard::DepsDeclared { eq } => {
                cand.row()
                    .and_then(|r| r.effect)
                    .is_some_and(|i| i.has_deps_array)
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
                    Candidate::Hook { .. } => {
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
