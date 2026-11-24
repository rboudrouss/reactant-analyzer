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
    /// The files matching the declared globs never run in the JavaScript
    /// realm of the files that read the accessor fact: they are left out of
    /// the program-wide join of the per-file facts
    /// ([`crate::ir::ModuleTable::accessor_names`], #379). A valued
    /// assumption: it is declared with its globs.
    ForeignRealmFiles,
}

impl Assumption {
    /// Every assumption, in the order they are stated.
    pub const ALL: &'static [Assumption] = &[
        Assumption::PlainObjectsRunNoAccessor,
        Assumption::ForeignRealmFiles,
    ];

    /// The name the config and `--assume` spell it with.
    pub fn name(self) -> &'static str {
        match self {
            Assumption::PlainObjectsRunNoAccessor => "plainObjectsRunNoAccessor",
            Assumption::ForeignRealmFiles => "foreignRealmFiles",
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Assumptions {
    /// No accessor of the program is ever installed on `Object.prototype`,
    /// and no prototype is ever set on a props or ref object. Recovers the
    /// findings a polyfill's variable-named getter hides; misses a loop that
    /// really runs through such a getter.
    #[serde(default)]
    pub plain_objects_run_no_accessor: bool,
    /// Globs, matched against each file's path relative to the project root,
    /// of the files that do not share a JavaScript realm with the app
    /// (polyfills, server code, SDKs, tests). Those files are still analysed
    /// and reported, and the reads in them still see every accessor of the
    /// program; the reads in the other files no longer see the accessors
    /// these install. Recovers the findings such an accessor hides; misses a
    /// loop through one that the app really reads. Empty by default.
    #[serde(default)]
    pub foreign_realm_files: Vec<String>,
}

impl Assumptions {
    /// Does the run declare `a`?
    pub fn holds(&self, a: Assumption) -> bool {
        match a {
            Assumption::PlainObjectsRunNoAccessor => self.plain_objects_run_no_accessor,
            Assumption::ForeignRealmFiles => !self.foreign_realm_files.is_empty(),
        }
    }

    /// Declare the flag assumption `a`. A valued one is declared with its
    /// value ([`Assumptions::declare_files`], [`Assumptions::declare_arg`]).
    pub fn declare(&mut self, a: Assumption) {
        match a {
            Assumption::PlainObjectsRunNoAccessor => self.plain_objects_run_no_accessor = true,
            Assumption::ForeignRealmFiles => {
                unreachable!("`foreignRealmFiles` is declared with its globs")
            }
        }
    }

    /// Declare that the files matching `glob` share no realm with the app.
    /// A glob already declared is not stated twice.
    pub fn declare_files(&mut self, glob: &str) {
        if !self.foreign_realm_files.iter().any(|g| g == glob) {
            self.foreign_realm_files.push(glob.to_string());
        }
    }

    /// Declare what one `--assume` argument says: `name` for a flag
    /// assumption, `name=value` for a valued one. A misspelt name, a value
    /// where none is taken and a missing one are errors: a premise must not
    /// run as no premise.
    pub fn declare_arg(&mut self, arg: &str) -> Result<(), String> {
        let (name, value) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (arg, None),
        };
        let a = Assumption::parse(name)?;
        match (a, value) {
            (Assumption::PlainObjectsRunNoAccessor, None) => self.declare(a),
            (Assumption::PlainObjectsRunNoAccessor, Some(_)) => {
                return Err(format!("`{name}` takes no value"));
            }
            (Assumption::ForeignRealmFiles, Some(glob)) if !glob.is_empty() => {
                self.declare_files(glob)
            }
            (Assumption::ForeignRealmFiles, _) => {
                return Err(format!("`{name}` needs a glob: `{name}=<glob>`"));
            }
        }
        Ok(())
    }

    /// The declared assumptions, in [`Assumption::ALL`] order.
    pub fn in_force(&self) -> Vec<Assumption> {
        Assumption::ALL
            .iter()
            .copied()
            .filter(|a| self.holds(*a))
            .collect()
    }

    /// What the declared assumptions say, as the human output states them,
    /// in [`Assumption::ALL`] order.
    pub fn statements(&self) -> Vec<String> {
        self.in_force()
            .into_iter()
            .map(|a| match a {
                Assumption::PlainObjectsRunNoAccessor => {
                    "plain objects run no accessor of the program".to_string()
                }
                Assumption::ForeignRealmFiles => format!(
                    "files matching {} do not share a realm with the app",
                    self.foreign_realm_files.join(", ")
                ),
            })
            .collect()
    }

    /// What the declarations of `other` add to these: the CLI's `--assume`
    /// over the config's section (ADR-022 §5; an assumption is turn-on-only,
    /// like the boolean flags, and a list adds to the list).
    pub fn merge(&mut self, other: &Assumptions) {
        self.plain_objects_run_no_accessor |= other.plain_objects_run_no_accessor;
        for glob in &other.foreign_realm_files {
            self.declare_files(glob);
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
        assert!(err.contains("foreignRealmFiles"), "{err}");
    }

    #[test]
    fn nothing_is_declared_by_default() {
        assert!(Assumptions::default().in_force().is_empty());
        assert!(Assumptions::default().statements().is_empty());
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

    #[test]
    fn globs_add_up_without_repeating() {
        let mut a = Assumptions::default();
        a.declare_arg("foreignRealmFiles=**/polyfills/**").unwrap();
        a.declare_arg("foreignRealmFiles=apps/server/**").unwrap();
        a.declare_arg("foreignRealmFiles=**/polyfills/**").unwrap();
        let mut config = Assumptions::default();
        config.declare_files("apps/server/**");
        config.declare_files("**/*.test.*");
        a.merge(&config);
        assert_eq!(
            a.foreign_realm_files,
            ["**/polyfills/**", "apps/server/**", "**/*.test.*"]
        );
        assert_eq!(a.in_force(), [Assumption::ForeignRealmFiles]);
        assert_eq!(
            a.statements(),
            [
                "files matching **/polyfills/**, apps/server/**, **/*.test.* do not share a realm with the app"
            ]
        );
    }

    #[test]
    fn an_argument_is_a_name_or_a_name_and_its_value() {
        let mut a = Assumptions::default();
        a.declare_arg("plainObjectsRunNoAccessor").unwrap();
        assert!(a.plain_objects_run_no_accessor);
        for bad in [
            "plainObjectsRunNoAccessor=true",
            "foreignRealmFiles",
            "foreignRealmFiles=",
            "foreignRealmFile=a/**",
        ] {
            assert!(Assumptions::default().declare_arg(bad).is_err(), "{bad}");
        }
    }
}
