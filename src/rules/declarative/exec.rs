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

use super::entity::{DepEntity, EntityCtx, EntityVal, HookRow, SetterEntity};
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
                                self.eval(&e, &row, Some(Bound::Dep(&dep)), &mut out);
                            }
                        }
                        Some(EdgeName::BodySetterCalls) => {
                            for setter in e.body_setters(&row) {
                                self.eval(&e, &row, Some(Bound::Setter(&setter)), &mut out);
                            }
                        }
                        None => self.eval(&e, &row, None, &mut out),
                    }
                }
            }
            ResolvedAnchor::RenderSetterCalls => {
                for setter in e.render_setters() {
                    self.eval_render_setter(&e, &setter, &mut out);
                }
            }
        }
        out
    }
}

impl TierARule {
    /// Evaluate guards for one (hook anchor, bound entity) candidate; emit on
    /// full pass. Guards run in author order and short-circuit.
    fn eval(
        &self,
        e: &EntityCtx<'_>,
        row: &HookRow<'_>,
        bound: Option<Bound<'_, '_>>,
        out: &mut Vec<Diagnostic>,
    ) {
        let mut proofs: Vec<Proof> = Vec::new();

        for guard in &self.def.guards {
            let pass = match guard {
                // The `bound` matches below are exhaustive, not refutable
                // `let`s: a new edge would otherwise validate, load, and
                // silently emit nothing.
                ResolvedGuard::Stability { names, negated, .. } => match bound.as_ref() {
                    Some(Bound::Dep(dep)) => names.contains(&e.dep_verdict(dep)) != *negated,
                    Some(Bound::Setter(_)) | None => {
                        unreachable!("validated: `stability` binds a deps entry")
                    }
                },
                ResolvedGuard::InDeps { negate, .. } => match bound.as_ref() {
                    Some(Bound::Setter(setter)) => {
                        let in_deps = setter
                            .slot
                            .is_some_and(|slot| e.dep_slots(row).contains(&slot));
                        in_deps != *negate
                    }
                    Some(Bound::Dep(_)) | None => {
                        unreachable!("validated: `in_deps` binds a body setter call")
                    }
                },
                ResolvedGuard::Text {
                    of,
                    field,
                    one_of,
                    prefix,
                } => text_matches(
                    e.field_raw(&entity_at(*of, row, bound.as_ref()), *field),
                    one_of,
                    prefix,
                ),
                ResolvedGuard::Count(cmp) => {
                    let len = row.effect.map_or(0, |i| i.declared_deps.len()) as u64;
                    match cmp {
                        CountCmp::MoreThan(n) => len > *n,
                        CountCmp::LessThan(n) => len < *n,
                        CountCmp::Equals(n) => len == *n,
                    }
                }
                ResolvedGuard::DepsDeclared { eq } => {
                    row.effect.is_some_and(|i| i.has_deps_array) == *eq
                }
                ResolvedGuard::Must { kind, els, .. } => {
                    match (self.certify(e, row, bound.as_ref(), *kind), els) {
                        (Some(p), _) => {
                            proofs.push(p);
                            true
                        }
                        (None, ElseBehavior::Keep) => true,
                        (None, ElseBehavior::Drop) => false,
                    }
                }
            };
            if !pass {
                return;
            }
        }

        let message: String = self
            .def
            .message
            .iter()
            .map(|seg| match seg {
                Segment::Lit(s) => s.clone(),
                Segment::Field(r, f) => e.render_field(&entity_at(*r, row, bound.as_ref()), *f),
            })
            .collect();
        let range = match bound.as_ref() {
            Some(Bound::Setter(s)) => s.span,
            Some(Bound::Dep(_)) | None => row.info.span,
        };
        out.push(self.emit(message, proofs, Some(row.info.label), range));
    }

    /// Same loop for the `render_setter_calls` anchor (no edges in v1; only
    /// `name` and `must_dominates_all_exits` guards survive validation).
    fn eval_render_setter(
        &self,
        e: &EntityCtx<'_>,
        setter: &SetterEntity,
        out: &mut Vec<Diagnostic>,
    ) {
        let mut proofs: Vec<Proof> = Vec::new();

        for guard in &self.def.guards {
            let pass = match guard {
                ResolvedGuard::Text {
                    field,
                    one_of,
                    prefix,
                    ..
                } => text_matches(
                    e.field_raw(&EntityVal::Setter(setter), *field),
                    one_of,
                    prefix,
                ),
                ResolvedGuard::Must {
                    kind: MustKind::DominatesAllExits,
                    els,
                    ..
                } => {
                    let proof = setter.block_id.and_then(|b| match e.exit_dom().certify(b) {
                        MustResult::All(c) => Some(Proof::Dominates(c)),
                        _ => None,
                    });
                    match (proof, els) {
                        (Some(p), _) => {
                            proofs.push(p);
                            true
                        }
                        (None, ElseBehavior::Keep) => true,
                        (None, ElseBehavior::Drop) => false,
                    }
                }
                _ => unreachable!("validated: guard not applicable to render_setter_calls"),
            };
            if !pass {
                return;
            }
        }

        let message: String = self
            .def
            .message
            .iter()
            .map(|seg| match seg {
                Segment::Lit(s) => s.clone(),
                Segment::Field(_, f) => e.render_field(&EntityVal::Setter(setter), *f),
            })
            .collect();
        out.push(self.emit(message, proofs, setter.slot, setter.span));
    }

    /// Run the must-primitive backing `kind` for this finding's subject.
    fn certify(
        &self,
        e: &EntityCtx<'_>,
        row: &HookRow<'_>,
        bound: Option<&Bound<'_, '_>>,
        kind: MustKind,
    ) -> Option<Proof> {
        match kind {
            MustKind::SetterOnAllPaths => {
                let setter = match bound {
                    Some(Bound::Setter(s)) => s,
                    Some(Bound::Dep(_)) | None => {
                        unreachable!("validated: `must_setter_on_all_paths` binds a body setter")
                    }
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
            MustKind::DominatesAllExits => unreachable!("validated: render anchor only"),
            MustKind::InitCallsSetter => {
                let init = match row.entry {
                    Some(HookEntry::State { init, .. } | HookEntry::Ref { init, .. }) => init,
                    _ => return None,
                };
                match must_init_calls_setter(init, &e.setter_vars) {
                    MustResult::All(c) => Some(Proof::Init(c)),
                    _ => None,
                }
            }
            MustKind::HookIsConditional => e
                .conditional()
                .get(&row.info.label)
                .cloned()
                .map(Proof::Conditional),
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

/// Resolve a guard/template subject to its entity value.
fn entity_at<'a, 'b>(
    r: BindRef,
    row: &'b HookRow<'a>,
    bound: Option<&'b Bound<'a, 'b>>,
) -> EntityVal<'a, 'b> {
    match r {
        BindRef::Anchor => EntityVal::Hook(row),
        BindRef::Bound => match bound.expect("validated: binding exists") {
            Bound::Setter(s) => EntityVal::Setter(s),
            Bound::Dep(d) => EntityVal::Dep(d),
        },
    }
}

/// Positive-only field matching (ADR-023): an absent value **fails**. A
/// negative form would let an unknown value pass a guard and, combined with a
/// must-guard, carry an Error on a candidate whose field we never resolved.
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
