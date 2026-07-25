//! The diagnostic type and its severity seal (ADR-021 §2).
//!
//! `Diagnostic` lives in this **leaf** module so that its `severity` field and
//! the free constructors (`new`/`with_severity`) are invisible to every rule
//! module. Rust privacy is downward: an item private to a module is visible to
//! that module's *descendants* — so when `Diagnostic` lived in `rules/mod.rs`,
//! every rule submodule could still call `Diagnostic::new(..).with_severity
//! (Severity::Error)` or write `d.severity = Severity::Error` through the
//! then-`pub` field, silently bypassing the proof discipline. In a leaf module
//! the rules are siblings, not descendants: the only doors are
//! [`Diagnostic::error`] (requires a [`Certified`] proof), [`Diagnostic::warn`]
//! and [`Diagnostic::info`]. Severity is read through [`Diagnostic::severity`].

use crate::ir::{
    SourceRange,
    types::{HookLabel, Var},
};

use super::query::Certified;
use super::witness::{Note, Step};

/// Confidence level of a diagnostic.
///
/// - `Error`   violation on ALL execution paths.
/// - `Warning` possible but uncertain (conditional path or over-approx).
/// - `Info`    known analysis limitation (widening, depth cap). Hidden by default; show with --info.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    Error,
    #[default]
    Warning,
    Info,
}

/// Finding produced by a rule against the fixpoint analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// Private (ADR-021): the seal. Set only by `error`/`warn`/`info`; read
    /// through [`Diagnostic::severity`]. A `pub` field here would let any code
    /// forge an Error by mutation or struct literal.
    severity: Severity,
    pub rule: &'static str,
    pub message: String,
    /// Hook label most directly involved, if any.
    pub hook_label: Option<HookLabel>,
    /// Variable name most directly involved, if any.
    pub var: Option<Var>,
    /// Source location of the primary finding, if available.
    pub range: Option<SourceRange>,
    /// Secondary evidence items explaining the causal chain.
    pub notes: Vec<Note>,
}

impl Diagnostic {
    /// Private to this leaf module (ADR-021): the rule-facing constructors are
    /// [`Diagnostic::error`] (needs a `Certified` proof), [`Diagnostic::warn`],
    /// and [`Diagnostic::info`].
    fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::default(),
            rule,
            message: message.into(),
            hook_label: None,
            var: None,
            range: None,
            notes: vec![],
        }
    }

    /// Private to this leaf module (ADR-021): severity is set by construction
    /// via `error`/`warn`/`info`.
    fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// The finding's confidence level. Read-only: severity is fixed at
    /// construction (`error`/`warn`/`info`) and cannot be reassigned.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn with_label(mut self, label: HookLabel) -> Self {
        self.hook_label = Some(label);
        self
    }

    pub fn with_var(mut self, var: impl Into<Var>) -> Self {
        self.var = Some(var.into());
        self
    }

    pub fn with_range(mut self, range: SourceRange) -> Self {
        self.range = Some(range);
        self
    }

    /// Append a typed witness step (ADR-019). `name` maps a state slot to its
    /// user-facing name — pass a closure over the rule's `state_slot_name`
    /// table, or [`super::witness::fallback_name`] when no table applies.
    pub fn with_step(
        mut self,
        step: Step,
        hook_label: Option<HookLabel>,
        range: Option<SourceRange>,
        name: &dyn Fn(HookLabel) -> String,
    ) -> Self {
        self.notes.push(Note {
            message: step.render(name),
            step,
            hook_label,
            range,
        });
        self
    }

    /// Append pre-built witness notes (from the `witness` producers —
    /// [`super::witness::chase_value`], [`super::witness::slot_history`], …).
    pub fn with_notes(mut self, notes: Vec<Note>) -> Self {
        self.notes.extend(notes);
        self
    }

    /// The **only** constructor of an `Error` (ADR-021 §2). Builds the finding
    /// *from* a proof: the certified evidence's span/label/witness ride along, so
    /// they need not be re-threaded by hand. A `May<_>`/`MustResult::Some` value
    /// has no `Certified` to pass here — Error-on-may is a type error.
    ///
    /// Further `.with_*` builders may still refine/override the absorbed fields.
    pub fn error<E>(rule: &'static str, proof: Certified<E>, message: impl Into<String>) -> Self {
        let prov = proof.provenance();
        Diagnostic {
            severity: Severity::Error,
            rule,
            message: message.into(),
            hook_label: prov.hook_label,
            var: None,
            range: prov.range,
            notes: prov.notes.clone(),
        }
    }

    /// A Warning: a safe over-claim (conditional path / over-approximation). Free
    /// to construct — a Warning asserts no MUST fact.
    pub fn warn(rule: &'static str, message: impl Into<String>) -> Self {
        Diagnostic::new(rule, message).with_severity(Severity::Warning)
    }

    /// An Info: a known analysis limitation. Makes no must/may claim; hidden
    /// unless `--info`.
    pub fn info(rule: &'static str, message: impl Into<String>) -> Self {
        Diagnostic::new(rule, message).with_severity(Severity::Info)
    }
}
