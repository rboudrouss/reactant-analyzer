//! Location grouping (#129): a finding's identity is its source location, not
//! the component that inlined it.
//!
//! `program_result.components` is keyed by component, so a custom hook inlined
//! into 87 components honestly produces its finding 87 times — once per
//! consumer, each row carrying its own component and the *hook's* file and
//! line. Nothing collapsed that on the way out, so the human report printed
//! the same source line 87 times and the summary counted it 87 times. Measured
//! on the 2026-09-02 corpus: 6,322 reported findings over 1,170 distinct
//! locations, 81% repetition.
//!
//! The grouping is a *display* fact and lives here rather than in the rule
//! pass: the JSON rows stay per-component (a consumer that wants per-component
//! attribution has it today), and `--fail-on` reads the unfiltered counts, so
//! nothing about which findings exist changes.

use std::collections::HashMap;

use crate::ir::FileId;
use crate::rules::Severity;

use super::report::ComponentReport;

/// The key a finding is grouped by — exactly the key the corpus measurement
/// used. `None` for the position when the diagnostic carries no range: a row
/// whose location is unknown is never claimed to be the same as another's.
type Key<'a> = (&'a str, Option<(FileId, u32, u32)>, &'a str);

/// Which components share each finding, and where each row should render.
pub struct LocationIndex {
    /// `roles[component][diagnostic]` — `Some(group)` when this row is the
    /// canonical one for its location, `None` when it repeats one printed
    /// earlier under a component sorted before it.
    roles: Vec<Vec<Option<usize>>>,
    /// Component indices sharing a location, canonical one first.
    groups: Vec<Vec<usize>>,
    /// Distinct-location counts — what the human summary reports.
    pub errors: usize,
    pub warnings: usize,
}

impl LocationIndex {
    pub fn build(components: &[ComponentReport]) -> Self {
        let mut seen: HashMap<Key, usize> = HashMap::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut roles: Vec<Vec<Option<usize>>> = Vec::with_capacity(components.len());
        let (mut errors, mut warnings) = (0usize, 0usize);

        for (ci, comp) in components.iter().enumerate() {
            let mut row = Vec::with_capacity(comp.diagnostics.len());
            for d in &comp.diagnostics {
                let key: Key = (
                    d.rule.as_ref(),
                    d.range.map(|r| (r.file, r.line, r.col)),
                    d.message.as_str(),
                );
                // A row with no range is its own group: two positions we
                // cannot name are not evidence of one location.
                let known = key.1.is_some();
                match seen.get(&key).copied().filter(|_| known) {
                    Some(g) => {
                        groups[g].push(ci);
                        row.push(None);
                    }
                    None => {
                        let g = groups.len();
                        groups.push(vec![ci]);
                        if known {
                            seen.insert(key, g);
                        }
                        match d.severity() {
                            Severity::Error => errors += 1,
                            Severity::Warning => warnings += 1,
                            Severity::Info => {}
                        }
                        row.push(Some(g));
                    }
                }
            }
            roles.push(row);
        }

        LocationIndex {
            roles,
            groups,
            errors,
            warnings,
        }
    }

    /// The components sharing the row at `(component, diagnostic)`, canonical
    /// one first — or `None` when that row repeats one already printed.
    pub fn consumers(&self, component: usize, diagnostic: usize) -> Option<&[usize]> {
        self.roles[component][diagnostic].map(|g| self.groups[g].as_slice())
    }

    /// True when every visible diagnostic of this component repeats a row
    /// printed under an earlier component.
    pub fn all_repeats(&self, component: usize) -> bool {
        let row = &self.roles[component];
        !row.is_empty() && row.iter().all(Option::is_none)
    }
}
