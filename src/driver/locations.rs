//! Location grouping (#129, #271): a finding's identity is its source
//! location, not the component that inlined it.
//!
//! `program_result.components` is keyed by component, so a custom hook inlined
//! into 87 components honestly produces its finding 87 times — once per
//! consumer, each row carrying its own component and the *hook's* file and
//! line. Nothing collapsed that on the way out, so the report printed the same
//! source line 87 times and the summary counted it 87 times. Measured on the
//! 2026-09-02 corpus: 6,322 reported findings over 1,170 distinct locations,
//! 81% repetition; on PR #225, 1,386 JSON rows for one mantine line.
//!
//! The grouping is a *display* fact and lives here rather than in the rule
//! pass: both renderers print one row per group, naming the components it is
//! reported through, and `--per-component` makes every row its own group
//! again. `--fail-on` reads the unfiltered counts, so nothing about which
//! findings exist changes.

use std::collections::HashMap;

use crate::ir::FileId;
use crate::rules::{Diagnostic, Severity};

use super::report::ComponentReport;

/// The key a finding is grouped by — exactly the key the corpus measurement
/// uses, plus the rule. The message is part of it, so two findings that say
/// different things on one line stay two rows. The level is not: the group
/// is shown at its strongest row. `None` for the position when the diagnostic
/// carries no range: a row whose location is unknown is never claimed to be
/// the same as another's.
type Key<'a> = (&'a str, Option<(FileId, u32, u32)>, &'a str);

/// One printed row: the components reporting the same finding.
struct Group {
    /// `(component, diagnostic)` of every member, in report order.
    rows: Vec<(usize, usize)>,
    /// The member the row is printed as: the first of the strongest level,
    /// so a group never shows a weaker level than one of its rows — an Error
    /// keeps its own proof and witness, it is not a Warning promoted.
    rep: (usize, usize),
}

/// Which components share each finding, and where each row should render.
pub struct LocationIndex {
    /// `roles[component][diagnostic]` — `Some(group)` when this row is the
    /// one its group is printed as, `None` when another member is.
    roles: Vec<Vec<Option<usize>>>,
    groups: Vec<Group>,
    /// Row counts by the level each group is printed at — what the summaries
    /// report.
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
}

impl LocationIndex {
    /// Group the rows of `components`; under `per_component` every row is its
    /// own group.
    pub fn build(components: &[ComponentReport], per_component: bool) -> Self {
        let mut seen: HashMap<Key, usize> = HashMap::new();
        let mut groups: Vec<Group> = Vec::new();
        let mut roles: Vec<Vec<Option<usize>>> = components
            .iter()
            .map(|c| vec![None; c.diagnostics.len()])
            .collect();

        for (ci, comp) in components.iter().enumerate() {
            for (di, d) in comp.diagnostics.iter().enumerate() {
                let key: Key = (
                    d.rule.as_ref(),
                    d.range.map(|r| (r.file, r.line, r.col)),
                    d.message.as_str(),
                );
                // A row with no range is its own group: two positions we
                // cannot name are not evidence of one location.
                let groupable = key.1.is_some() && !per_component;
                match seen.get(&key).copied().filter(|_| groupable) {
                    Some(g) => {
                        let group = &mut groups[g];
                        group.rows.push((ci, di));
                        let (rc, rd) = group.rep;
                        if d.severity().rank() > components[rc].diagnostics[rd].severity().rank() {
                            group.rep = (ci, di);
                        }
                    }
                    None => {
                        if groupable {
                            seen.insert(key, groups.len());
                        }
                        groups.push(Group {
                            rows: vec![(ci, di)],
                            rep: (ci, di),
                        });
                    }
                }
            }
        }

        let (mut errors, mut warnings, mut infos) = (0usize, 0usize, 0usize);
        for (g, group) in groups.iter().enumerate() {
            let (ci, di) = group.rep;
            roles[ci][di] = Some(g);
            match components[ci].diagnostics[di].severity() {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                Severity::Info => infos += 1,
            }
        }

        LocationIndex {
            roles,
            groups,
            errors,
            warnings,
            infos,
        }
    }

    /// The components sharing the row at `(component, diagnostic)`, in report
    /// order — or `None` when another member of its group is the one printed.
    pub fn consumers(&self, component: usize, diagnostic: usize) -> Option<Vec<usize>> {
        self.roles[component][diagnostic].map(|g| {
            let mut comps: Vec<usize> = self.groups[g].rows.iter().map(|&(c, _)| c).collect();
            comps.dedup();
            comps
        })
    }

    /// True when every visible diagnostic of this component is printed as
    /// another component's row.
    pub fn all_repeats(&self, component: usize) -> bool {
        let row = &self.roles[component];
        !row.is_empty() && row.iter().all(Option::is_none)
    }

    /// Every printed row in report order: the diagnostic it is printed as,
    /// its component, and the components it is reported through.
    pub fn rows<'a>(
        &'a self,
        components: &'a [ComponentReport],
    ) -> impl Iterator<Item = (&'a Diagnostic, usize, Vec<usize>)> + 'a {
        components.iter().enumerate().flat_map(move |(ci, comp)| {
            comp.diagnostics
                .iter()
                .enumerate()
                .filter_map(move |(di, d)| Some((d, ci, self.consumers(ci, di)?)))
        })
    }
}

/// The components a row is reported through, the way `always-unstable-deps`
/// names the parents of an element: `` `<A>`, `<B>` and `<C>` ``, or a count
/// past [`MAX_NAMED`] (the full list is the JSON's `components`, and the
/// human `--trace`'s `in:` line). `None` for a row only one component reports.
pub fn through(names: &[&str]) -> Option<String> {
    let quoted = |n: &&str| format!("`<{n}>`");
    match names {
        [] | [_] => None,
        _ if names.len() > MAX_NAMED => {
            Some(format!("reported through {} components", names.len()))
        }
        [init @ .., last] => Some(format!(
            "reported through {} and {}",
            init.iter().map(quoted).collect::<Vec<_>>().join(", "),
            quoted(last)
        )),
    }
}

/// Up to this many components are named on the row; more are counted.
pub const MAX_NAMED: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FileTable, SourceRange};

    fn comp(name: &str, diagnostics: Vec<Diagnostic>) -> ComponentReport {
        ComponentReport {
            name: name.to_string(),
            file: None,
            hook_count: 1,
            diagnostics,
            safe_checks: vec![],
            suspended_safe_checks: 0,
        }
    }

    fn at(d: Diagnostic, line: u32) -> Diagnostic {
        let file = FileTable::default().intern(std::path::Path::new("useShared.ts"));
        d.with_range(SourceRange { file, line, col: 2 })
    }

    #[test]
    fn a_group_is_printed_at_its_strongest_row() {
        let comps = [
            comp("A", vec![at(Diagnostic::info("r", "m"), 3)]),
            comp("B", vec![at(Diagnostic::warn("r", "m"), 3)]),
            comp("C", vec![at(Diagnostic::info("r", "m"), 3)]),
        ];
        let idx = LocationIndex::build(&comps, false);
        let rows: Vec<_> = idx.rows(&comps).collect();
        assert_eq!(rows.len(), 1);
        let (d, ci, through) = &rows[0];
        assert_eq!((d.severity(), *ci), (Severity::Warning, 1));
        assert_eq!(through, &vec![0, 1, 2]);
        assert_eq!((idx.errors, idx.warnings, idx.infos), (0, 1, 0));
        assert!(idx.all_repeats(0) && idx.all_repeats(2) && !idx.all_repeats(1));
    }

    #[test]
    fn a_different_message_or_line_is_another_row() {
        let comps = [
            comp("A", vec![at(Diagnostic::warn("r", "m"), 3)]),
            comp("B", vec![at(Diagnostic::warn("r", "other"), 3)]),
            comp("C", vec![at(Diagnostic::warn("r", "m"), 4)]),
            comp(
                "D",
                vec![Diagnostic::warn("r", "m"), Diagnostic::warn("r", "m")],
            ),
        ];
        let idx = LocationIndex::build(&comps, false);
        assert_eq!(idx.rows(&comps).count(), 5);
    }

    #[test]
    fn per_component_keeps_every_row() {
        let comps = [
            comp("A", vec![at(Diagnostic::warn("r", "m"), 3)]),
            comp("B", vec![at(Diagnostic::warn("r", "m"), 3)]),
        ];
        let idx = LocationIndex::build(&comps, true);
        let rows: Vec<_> = idx.rows(&comps).map(|(_, c, t)| (c, t)).collect();
        assert_eq!(rows, vec![(0, vec![0]), (1, vec![1])]);
        assert_eq!(idx.warnings, 2);
    }

    #[test]
    fn a_row_names_up_to_three_components_and_counts_more() {
        assert_eq!(through(&["A"]), None);
        assert_eq!(
            through(&["A", "B"]).as_deref(),
            Some("reported through `<A>` and `<B>`")
        );
        assert_eq!(
            through(&["A", "B", "C"]).as_deref(),
            Some("reported through `<A>`, `<B>` and `<C>`")
        );
        assert_eq!(
            through(&["A", "B", "C", "D"]).as_deref(),
            Some("reported through 4 components")
        );
    }
}
