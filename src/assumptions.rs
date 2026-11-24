//! Assumptions are configuration (ADR-048): named statements about the
//! analysed program that the analysis takes as true when the run declares
//! them, and never otherwise.
//!
//! One type serves every frontend: it is the `assumptions` section of
//! `reactant.config.json` (misspelt names are errors), what `--assume <name>`
//! turns on, what the engine reads ([`crate::engine::Config::assumptions`]),
//! and what both output formats state once per run.

use serde::Deserialize;

#[cfg(feature = "schema-gen")]
use schemars::JsonSchema;

/// One assumption a run may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assumption {
    /// No accessor of the program is ever installed on `Object.prototype`,
    /// and no prototype is ever set on a props or ref object: a member read
    /// off a plain object the library builds runs no code of the program
    /// ([`crate::ir::module::AccessorNames::may_run_off_plain`], #336).
    PlainObjectsRunNoAccessor,
}

impl Assumption {
    /// Every assumption, in the order they are stated.
    pub const ALL: &'static [Assumption] = &[Assumption::PlainObjectsRunNoAccessor];

    /// The name the config and `--assume` spell it with.
    pub fn name(self) -> &'static str {
        match self {
            Assumption::PlainObjectsRunNoAccessor => "plainObjectsRunNoAccessor",
        }
    }

    /// What it says, as the human output states it.
    pub fn statement(self) -> &'static str {
        match self {
            Assumption::PlainObjectsRunNoAccessor => "plain objects run no accessor of the program",
        }
    }

    /// The assumption named `name`, or the error listing the valid names.
    pub fn parse(name: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|a| a.name() == name)
            .ok_or_else(|| {
                let valid: Vec<&str> = Self::ALL.iter().map(|a| a.name()).collect();
                format!(
                    "unknown assumption `{name}`, expected one of: {}",
                    valid.join(", ")
                )
            })
    }
}

/// The assumptions a run declares: all off unless declared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Assumptions {
    /// No accessor of the program is ever installed on `Object.prototype`,
    /// and no prototype is ever set on a props or ref object. Recovers the
    /// findings a polyfill's variable-named getter hides; misses a loop that
    /// really runs through such a getter.
    #[serde(default)]
    pub plain_objects_run_no_accessor: bool,
}

impl Assumptions {
    /// Does the run declare `a`?
    pub fn holds(&self, a: Assumption) -> bool {
        match a {
            Assumption::PlainObjectsRunNoAccessor => self.plain_objects_run_no_accessor,
        }
    }

    /// Declare `a`.
    pub fn declare(&mut self, a: Assumption) {
        match a {
            Assumption::PlainObjectsRunNoAccessor => self.plain_objects_run_no_accessor = true,
        }
    }

    /// The declared assumptions, in [`Assumption::ALL`] order.
    pub fn in_force(&self) -> Vec<Assumption> {
        Assumption::ALL
            .iter()
            .copied()
            .filter(|a| self.holds(*a))
            .collect()
    }

    /// What the declarations of `other` add to these: the CLI's `--assume`
    /// over the config's section (ADR-022 §5; an assumption is turn-on-only,
    /// like the boolean flags).
    pub fn merge(&mut self, other: &Assumptions) {
        for a in other.in_force() {
            self.declare(a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_ones_list_the_valid() {
        for a in Assumption::ALL {
            assert_eq!(Assumption::parse(a.name()), Ok(*a));
        }
        let err = Assumption::parse("plainObjectRunNoAccessor").unwrap_err();
        assert!(err.contains("plainObjectsRunNoAccessor"), "{err}");
    }

    #[test]
    fn nothing_is_declared_by_default() {
        assert!(Assumptions::default().in_force().is_empty());
    }

    #[test]
    fn merge_only_turns_on() {
        let mut a = Assumptions::default();
        let mut on = Assumptions::default();
        on.declare(Assumption::PlainObjectsRunNoAccessor);
        a.merge(&on);
        assert_eq!(a.in_force(), [Assumption::PlainObjectsRunNoAccessor]);
        a.merge(&Assumptions::default());
        assert_eq!(a.in_force(), [Assumption::PlainObjectsRunNoAccessor]);
    }
}
