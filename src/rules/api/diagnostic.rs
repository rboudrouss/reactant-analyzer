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

use std::borrow::Cow;

use crate::ir::{
    SourceRange,
    types::{HookLabel, Var},
};

use super::query::Certified;
use super::witness::{Note, Step};

/// Confidence level of a diagnostic.
///
/// - `Error`   the defect is certain whenever the flagged code runs. Built only
///   from a proof of the whole claim, not of one of its conjuncts (#142).
/// - `Warning` a possible defect (conditional path, over-approximation), or a
///   certain fact whose cost is not (a fresh reference, one wasted render).
/// - `Info`    not actionable without context: a known analysis limitation
///   (widening, depth cap), or a pattern that looks intentional (a seed-once
///   prop name, a cheap pure initializer). Hidden by default; show with --info.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    Error,
    #[default]
    Warning,
    Info,
}

impl Severity {
    /// Lattice rank for the `⊓` clamp (ADR-022 §3): Error > Warning > Info.
    /// Distinct from the enum discriminant, whose order (Error < Warning <
    /// Info) the deterministic output sort depends on.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Severity::Error => 2,
            Severity::Warning => 1,
            Severity::Info => 0,
        }
    }
}

/// Finding produced by a rule against the fixpoint analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// Private (ADR-021): the seal. Set only by `error`/`warn`/`info`; read
    /// through [`Diagnostic::severity`]. A `pub` field here would let any code
    /// forge an Error by mutation or struct literal.
    severity: Severity,
    /// Diagnostic name. `Cow`: native rules pass string literals (borrowed,
    /// zero-cost), dynamically loaded rules own their `pack/rule` id.
    pub rule: Cow<'static, str>,
    pub message: String,
    /// Hook label most directly involved, if any.
    pub hook_label: Option<HookLabel>,
    /// Variable name most directly involved, if any.
    pub var: Option<Var>,
    /// Source location of the primary finding, if available.
    pub range: Option<SourceRange>,
    /// Secondary evidence items explaining the causal chain.
    pub notes: Vec<Note>,
    /// The setters a hand-over row stands for (#377), in the order its
    /// message names them, or the writes a dispatch row's listeners make;
    /// empty on every other row.
    pub setters: Vec<SetterRef>,
    /// A dispatch row's call (#407): the row is that call's, at `range`, and
    /// `setters` are what the listeners it runs write.
    pub dispatch: Option<DispatchRef>,
    /// The cycles a loop row stands for, every one, in the order its message
    /// names them ([`cycles_message`]); empty on every other row.
    pub cycles: Vec<CycleRef>,
    /// The family a loop row is one of at its site: rows of one rule and
    /// family at one site are one row, listing their cycles, within a
    /// component and across the components it is reported through
    /// ([`merge_cycles`]). `None`: grouped only with a row saying the same.
    pub family: Option<&'static str>,
}

/// One cycle a loop row stands for: the component it is reported in, the
/// path its message names, and its steps in that order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CycleRef {
    /// The component the cycle is reported in, as the report names it.
    pub component: String,
    /// The cycle as the message names it, from that component
    /// (`` `a` → `b` of `P` → `a` ``).
    pub path: String,
    pub steps: Vec<CycleStep>,
    /// What a row of this cycle alone says.
    pub sentence: String,
    /// What it says of the step the analysis could not prove; `None` for a
    /// proven cycle.
    pub unproven: Option<String>,
}

/// One step of a cycle: a write the carrier's effect (or render) makes, which
/// a change of `from` re-runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CycleStep {
    /// The component whose effect or render makes the write.
    pub carrier: String,
    pub from: SlotRef,
    /// The state the step writes.
    pub to: SlotRef,
    /// Where it writes it.
    pub site: Option<SourceRange>,
    /// How a change of `from` re-runs the carrier (`dep`, `versioned`,
    /// `props`, `every-render`, `read`, `mount`, `render`).
    pub reruns: &'static str,
    /// Why the step is not proven to happen at every round, a kind and its
    /// sentence, from the engine's own reason (`ChurnEdge::why`); `None` for
    /// a step proven to.
    pub unproven: Option<(&'static str, String)>,
}

/// A state of a cycle step.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotRef {
    /// The component that owns it, as the report names it; `None` for a
    /// library store (ADR-046).
    pub component: Option<String>,
    /// Its name as a message prints it, unquoted (`count`, `state #7`).
    pub state: String,
}

impl CycleRef {
    /// A loop row's message names this many cycles and counts the rest.
    pub const NAMED: usize = 3;
}

/// The message of a loop row standing for `cycles`, in their order: the
/// first one's sentence and what it could not prove, then the others,
/// named up to [`CycleRef::NAMED`] in all and counted past, each with the
/// component it is reported in where that is not the first one's.
pub fn cycles_message(cycles: &[CycleRef]) -> String {
    let Some(first) = cycles.first() else {
        return String::new();
    };
    let mut msg = first.sentence.clone();
    if let Some(u) = &first.unproven {
        msg.push_str(". Not proven: ");
        msg.push_str(u);
    }
    let rest = &cycles[1..];
    if rest.is_empty() {
        return msg;
    }
    let named: Vec<String> = rest
        .iter()
        .take(CycleRef::NAMED - 1)
        .map(|c| match c.component == first.component {
            true => format!("({})", c.path),
            false => format!("({}) in `{}`", c.path, c.component),
        })
        .collect();
    let list = match rest.len() - named.len() {
        0 => match named.as_slice() {
            [one] => one.clone(),
            [init @ .., last] => format!("{} and {last}", init.join(", ")),
            [] => String::new(),
        },
        more => format!("{} and {more} more", named.join(", ")),
    };
    let more = match rest.len() {
        1 => "1 more cycle goes".to_string(),
        n => format!("{n} more cycles go"),
    };
    msg.push_str(&format!(". {more} through this effect: {list}"));
    msg
}

/// One row of `rows`, loop rows of one rule and family at one site: the
/// first of the strongest level, standing for every cycle of them, each
/// (component, path, sites) once, in that order, its message rebuilt from
/// them. What the rule does with a component's rows and the report with a
/// site's rows across components: the same facts, one row.
pub fn merge_cycles<'d>(rows: impl IntoIterator<Item = &'d Diagnostic>) -> Option<Diagnostic> {
    let mut rows = rows.into_iter();
    let mut row = rows.next()?.clone();
    for d in rows {
        if d.severity.rank() > row.severity.rank() {
            let cycles = std::mem::take(&mut row.cycles);
            row = d.clone();
            row.cycles.extend(cycles);
        } else {
            row.cycles.extend(d.cycles.iter().cloned());
        }
    }
    // Two cycles alike in all three are one finding to a reader: the copies
    // of a hook a component inlines twice, whose slots differ only by an id
    // no output shows (one row of identical messages before, #129).
    row.cycles.sort_by(|a, b| a.order().cmp(&b.order()));
    row.cycles.dedup_by(|a, b| a.order() == b.order());
    row.message = cycles_message(&row.cycles);
    Some(row)
}

impl CycleRef {
    /// The order a loop row lists its cycles in, and what tells two apart:
    /// (component, path, the sites of its steps).
    pub fn order(&self) -> (&str, &str, Vec<Option<SourceRange>>) {
        (
            &self.component,
            &self.path,
            self.steps.iter().map(|s| s.site).collect(),
        )
    }
}

/// The call of a dispatch row (#407).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DispatchRef {
    /// The call as the source spells it (`inputRef.current.focus()`).
    pub call: String,
    /// The writes may run at any time, a render included: the call sits in
    /// code handed to a callee with no timing summary. Else they run in the
    /// render the call is made in.
    pub unknown: bool,
}

/// One setter a hand-over row stands for (#377): a statement that hands
/// values to code the analysis does not see is one row, which lists every
/// setter they carry, each as the message would name it alone.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SetterRef {
    /// The setter's name, printed; `None` for a slot the source binds to no
    /// name.
    pub name: Option<String>,
    /// The component whose state it sets, when that is not the row's own
    /// (`cross-setter-in-render`).
    pub parent: Option<String>,
    /// What it was handed through, printed, when that is not the setter.
    pub through: Option<String>,
    /// The name the component holds it by at the hand-over, for a parent's
    /// setter: what a one-setter row's `var` says.
    pub var: Option<Var>,
}

impl SetterRef {
    /// A hand-over row's message names up to this many setters and counts
    /// the rest.
    pub const NAMED: usize = 3;

    /// The setter in a list: `` `setN` ``, `` `setN` of `Parent` ``, and
    /// `` through `payload` `` after it when `through` asks.
    pub fn entry(&self, through: bool) -> String {
        let name = self
            .name
            .as_deref()
            .map_or_else(|| "a state setter".to_string(), |n| format!("`{n}`"));
        let parent = self
            .parent
            .as_deref()
            .map_or_else(String::new, |p| format!(" of `{p}`"));
        let via = match (&self.through, through) {
            (Some(t), true) => format!(" through `{t}`"),
            _ => String::new(),
        };
        format!("{name}{parent}{via}")
    }
}

impl Diagnostic {
    /// Private to this leaf module (ADR-021): the rule-facing constructors are
    /// [`Diagnostic::error`] (needs a `Certified` proof), [`Diagnostic::warn`],
    /// and [`Diagnostic::info`].
    fn new(rule: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::default(),
            rule: rule.into(),
            message: message.into(),
            hook_label: None,
            var: None,
            range: None,
            notes: vec![],
            setters: vec![],
            dispatch: None,
            cycles: vec![],
            family: None,
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

    /// Consumer override (ADR-022 §3): lower this finding to `ceiling` when its
    /// constructed severity exceeds it. Downgrade-only, hence sound to expose:
    /// Error is constructible only through [`Diagnostic::error`], so the
    /// constructed severity IS the polarity ceiling and `pin ⊓ polarity`
    /// reduces to a min — an upgrade request is a structural no-op.
    pub fn clamp(mut self, ceiling: Severity) -> Self {
        if ceiling.rank() < self.severity.rank() {
            self.severity = ceiling;
        }
        self
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

    /// A setter this hand-over row stands for (#377).
    pub fn with_setter(mut self, setter: SetterRef) -> Self {
        self.setters.push(setter);
        self
    }

    /// Append a typed witness step (ADR-019). `names` is the rule's
    /// [`RuleCtx::names`](super::query::RuleCtx::names): a closure over its
    /// `state_slot_name` table, or [`super::witness::fallback_name`] when no
    /// table applies.
    pub fn with_step(
        mut self,
        step: Step,
        hook_label: Option<HookLabel>,
        range: Option<SourceRange>,
        names: &super::witness::Names,
    ) -> Self {
        self.notes.push(Note {
            message: step.render(names),
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
    pub fn error<E>(
        rule: impl Into<Cow<'static, str>>,
        proof: Certified<E>,
        message: impl Into<String>,
    ) -> Self {
        let prov = proof.provenance();
        Diagnostic {
            severity: Severity::Error,
            rule: rule.into(),
            message: message.into(),
            hook_label: prov.hook_label,
            var: None,
            range: prov.range,
            notes: prov.notes.clone(),
            setters: vec![],
            dispatch: None,
            cycles: vec![],
            family: None,
        }
    }

    /// A Warning: a safe over-claim (conditional path / over-approximation). Free
    /// to construct — a Warning asserts no MUST fact.
    pub fn warn(rule: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Diagnostic::new(rule, message).with_severity(Severity::Warning)
    }

    /// An Info: a known analysis limitation, or a pattern that looks
    /// intentional. Makes no must/may claim; hidden unless `--info`.
    pub fn info(rule: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Diagnostic::new(rule, message).with_severity(Severity::Info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_lowers_above_ceiling() {
        let d = Diagnostic::warn("r", "m").clamp(Severity::Info);
        assert_eq!(d.severity(), Severity::Info);
    }

    #[test]
    fn clamp_upgrade_is_a_no_op() {
        // The soundness case: a consumer pin can never raise a finding above
        // its constructed (polarity-derived) severity.
        let d = Diagnostic::warn("r", "m").clamp(Severity::Error);
        assert_eq!(d.severity(), Severity::Warning);
        let d = Diagnostic::info("r", "m").clamp(Severity::Warning);
        assert_eq!(d.severity(), Severity::Info);
    }

    #[test]
    fn a_loop_row_names_its_first_cycles_and_counts_the_rest() {
        let cycle = |component: &str, path: &str| CycleRef {
            component: component.to_string(),
            path: path.to_string(),
            steps: vec![],
            sentence: format!("these effects may form a state-update cycle ({path})"),
            unproven: Some("the write of `a` is handed to package code".to_string()),
        };
        let one = [cycle("A", "`a` → `a`")];
        assert_eq!(
            cycles_message(&one),
            "these effects may form a state-update cycle (`a` → `a`). Not proven: the write \
             of `a` is handed to package code"
        );
        let five: Vec<CycleRef> = [
            "`a` → `a`",
            "`b` → `b`",
            "`c` → `c`",
            "`d` → `d`",
            "`e` → `e`",
        ]
        .iter()
        .enumerate()
        .map(|(i, p)| cycle(if i == 2 { "B" } else { "A" }, p))
        .collect();
        assert!(
            cycles_message(&five).ends_with(
                ". 4 more cycles go through this effect: (`b` → `b`), (`c` → `c`) in `B` and 2 \
                 more"
            ),
            "{}",
            cycles_message(&five)
        );
        // A merge keeps each cycle once, in (component, path) order, and
        // rebuilds the message from them.
        let mut a = Diagnostic::warn("infinite-loop", "x");
        a.cycles = vec![five[1].clone(), five[0].clone()];
        let mut b = Diagnostic::warn("infinite-loop", "y");
        b.cycles = vec![five[0].clone(), five[2].clone()];
        let m = merge_cycles([&a, &b]).unwrap();
        let paths: Vec<&str> = m.cycles.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["`a` → `a`", "`b` → `b`", "`c` → `c`"]);
        assert_eq!(m.message, cycles_message(&m.cycles));
    }

    #[test]
    fn clamp_at_own_level_is_identity() {
        let d = Diagnostic::warn("r", "m").clamp(Severity::Warning);
        assert_eq!(d.severity(), Severity::Warning);
    }
}
