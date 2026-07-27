//! The dynamic rule registry (ADR-022 §8).
//!
//! Replaces the direct `all_rules()` call: natives first, then pack rules in
//! registration order (config pack order × pack rule order) — deterministic
//! output ordering. Also owns the per-component rule pass, including the
//! consumer severity clamp (`pin ⊓ polarity`, ADR-022 §3) and the off/allow
//! filters, so every frontend (CLI, WASM) runs the exact same composition.
//!
//! Not to be confused with `crate::registry` (external hook summaries).
//!
//! Naming discipline: severity/off overrides are keyed by **diagnostic name**
//! (`Diagnostic::rule`, the namespace of `--rule`/`--ignore-rule` and
//! `RuleDoc`); options are keyed by **rule id** (`Rule::name()`). One rule may
//! emit several diagnostic names (`setter-in-render` also emits
//! `cross-setter-in-render`), which is why `"off"` filters findings at
//! emission and never skips a rule's execution — skipping by rule id would
//! silently swallow the *other* diagnostic name, a forbidden false negative.

use std::collections::{BTreeMap, BTreeSet};

use crate::engine::ProgramAnalysisResult;
use crate::ir::types::Symbol;

use super::api::diagnostic::{Diagnostic, Severity};
use super::api::query::{RuleConfig, RuleCtx};
use super::docs::{RULE_DOCS, RuleDoc};
use super::impls::AnalysisLimitInfo;
use super::{Rule, SafeCheck, all_rules};

/// Consumer overrides, already precedence-resolved by the frontend (ADR-022
/// §5: CLI beats config; the registry never sees raw flags).
#[derive(Debug, Clone, Default)]
pub struct RuleOverrides {
    /// Keyed by diagnostic name (severity/off) — options entries additionally
    /// require the key to be a pack rule id (natives declare no params in v1).
    pub entries: BTreeMap<String, OverrideEntry>,
    /// `--rule` allowlist; `None` = everything visible.
    pub allow: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct OverrideEntry {
    /// Drop this diagnostic name at emission.
    pub off: bool,
    /// Severity pin: findings are clamped down to this ceiling (never up —
    /// [`Diagnostic::clamp`] is downgrade-only by construction).
    pub ceiling: Option<Severity>,
    /// Per-rule options, delivered to the rule via [`RuleConfig`].
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// One component's rule-pass output: clamped, filtered, deterministically
/// sorted.
#[derive(Debug, Default)]
pub struct ComponentFindings {
    pub diagnostics: Vec<Diagnostic>,
    pub safe_checks: Vec<SafeCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// Override key matches no known diagnostic name.
    UnknownRule(String),
    /// Options addressed to a native rule — natives declare no params in v1
    /// (ADR-022 §4).
    OptionsOnNative(String),
    /// Options addressed to a diagnostic-only name (e.g.
    /// `cross-setter-in-render`), which is not a rule id.
    OptionsOnDiagnosticOnly(String),
    /// A rule or doc with this name is already registered.
    DuplicateName(String),
    /// Dynamically registered rules must be namespaced `pack/rule`; bare
    /// names are reserved for natives (ADR-022 §5).
    BareDynamicName(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::UnknownRule(n) => write!(
                f,
                "unknown rule `{n}` — run `reactant rules` for the list of valid names"
            ),
            RegistryError::OptionsOnNative(n) => {
                write!(f, "rule `{n}` is built-in and declares no options")
            }
            RegistryError::OptionsOnDiagnosticOnly(n) => write!(
                f,
                "`{n}` is a diagnostic name, not a rule id — options must target the rule"
            ),
            RegistryError::DuplicateName(n) => write!(f, "rule `{n}` is already registered"),
            RegistryError::BareDynamicName(n) => write!(
                f,
                "custom rule `{n}` must be namespaced `pack/rule` — bare names are reserved \
                 for built-in rules"
            ),
        }
    }
}

pub struct RuleRegistry {
    /// Natives first, then packs in registration order.
    rules: Vec<Box<dyn Rule>>,
    /// One doc per diagnostic name (16 native entries, then one per pack rule).
    docs: Vec<RuleDoc>,
    overrides: RuleOverrides,
}

impl RuleRegistry {
    /// The 14 native rules and the 16-entry doc table; no overrides.
    pub fn natives() -> Self {
        RuleRegistry {
            rules: all_rules(),
            docs: RULE_DOCS.to_vec(),
            overrides: RuleOverrides::default(),
        }
    }

    /// Append one dynamically loaded rule with its doc (ADR-022 §5 identity
    /// discipline: the id must be namespaced `pack/rule` and collide with
    /// nothing). Registration order is the output order contract (§8).
    pub fn register(&mut self, rule: Box<dyn Rule>, doc: RuleDoc) -> Result<(), RegistryError> {
        let id = rule.name().to_string();
        if !id.contains('/') {
            return Err(RegistryError::BareDynamicName(id));
        }
        if self.rules.iter().any(|r| r.name() == id) || self.doc(&id).is_some() {
            return Err(RegistryError::DuplicateName(id));
        }
        if doc.name != id.as_str() {
            // The doc is looked up by diagnostic name = rule id for Tier-A
            // rules; a mismatch would make `reactant explain` miss it.
            return Err(RegistryError::UnknownRule(doc.name.to_string()));
        }
        self.rules.push(rule);
        self.docs.push(doc);
        Ok(())
    }

    /// Install resolved overrides. Loud: every key must name a known
    /// diagnostic, and options must target a rule that can accept them.
    pub fn set_overrides(&mut self, overrides: RuleOverrides) -> Result<(), RegistryError> {
        for (name, entry) in &overrides.entries {
            if self.doc(name).is_none() {
                return Err(RegistryError::UnknownRule(name.clone()));
            }
            if !entry.options.is_empty() {
                if !self.rules.iter().any(|r| r.name() == name) {
                    return Err(RegistryError::OptionsOnDiagnosticOnly(name.clone()));
                }
                if !name.contains('/') {
                    return Err(RegistryError::OptionsOnNative(name.clone()));
                }
            }
        }
        if let Some(allow) = &overrides.allow {
            for name in allow {
                if self.doc(name).is_none() {
                    return Err(RegistryError::UnknownRule(name.clone()));
                }
            }
        }
        self.overrides = overrides;
        Ok(())
    }

    pub fn doc(&self, name: &str) -> Option<&RuleDoc> {
        self.docs.iter().find(|d| d.name == name)
    }

    pub fn docs(&self) -> impl Iterator<Item = &RuleDoc> {
        self.docs.iter()
    }

    /// Whether `name` survives the off/allow composition. Pure semantics:
    /// `off` always drops, `allow` always restricts. Softer compositions
    /// (an explicit `--rule X` resurrecting a config-`"off"` X, but never a
    /// `--ignore-rule X`) are resolved by the frontend when it *builds* the
    /// [`RuleOverrides`].
    fn visible(&self, name: &str) -> bool {
        !self.overrides.entries.get(name).is_some_and(|e| e.off)
            && self
                .overrides
                .allow
                .as_ref()
                .is_none_or(|a| a.contains(name))
    }

    fn options_for(&self, rule_id: &str) -> RuleConfig {
        self.overrides
            .entries
            .get(rule_id)
            .filter(|e| !e.options.is_empty())
            .map(|e| RuleConfig::new(e.options.clone()))
            .unwrap_or_default()
    }

    /// The whole per-component rule pass: per rule → ctx (with per-rule
    /// options) → check → `safe_check` fallback → clamp (§3, before the sort:
    /// severity is a sort key) → off/allow filter → deterministic sort.
    ///
    /// The Info visibility filter (`--info`) is a display concern and stays
    /// with the caller.
    pub fn check_component(
        &self,
        program: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> ComponentFindings {
        let mut diags: Vec<Diagnostic> = Vec::new();
        let mut safe_checks: Vec<SafeCheck> = Vec::new();
        for r in &self.rules {
            let ctx = RuleCtx::with_config(program, component, self.options_for(r.name()));
            let produced = r.check(&ctx);
            // `safe_check` is consulted on the *raw* output: a rule whose
            // findings were all filtered away still ran and found something —
            // it is not "verified safe".
            if produced.is_empty()
                && let Some(sc) = r.safe_check(&ctx)
            {
                safe_checks.push(sc);
            }
            diags.extend(produced.into_iter().map(|d| self.clamped(d)));
        }

        // A component where the analyzer admits it truncated (`analysis-limit`
        // says "FN possible") must not also publish `verified: …` universals
        // that the very same limit could falsify — an opaque hook may hide the
        // conditional call, the missing dep, the diverging effect. Read off the
        // *unfiltered* diagnostics on purpose: silencing the Info with
        // `--ignore-rule` hides the notice, it does not restore the guarantee.
        if diags.iter().any(|d| d.rule == AnalysisLimitInfo::NAME) {
            safe_checks.clear();
        }

        diags.retain(|d| self.visible(&d.rule));
        safe_checks.retain(|s| self.visible(s.rule));

        // Total order: rules iterate HashMaps internally, so same-key ties
        // (many `analysis-limit` Infos, several notes on one slot) come back
        // in a run-dependent order — tie-break on position, then content, so
        // consecutive runs are byte-identical (CI/bench diffing).
        //
        // The position key is `(file, line, col)`, not `(line, col)`: a
        // component whose hooks are inlined from several files (ADR-013) holds
        // anchors from all of them, so bare line numbers both interleave the
        // origins and leave a genuine tie — two findings at the same line:col
        // of two different hook files — to HashMap order. `is_none()` leads so
        // range-less findings stay last, as under the old `u32::MAX` sentinel.
        let loc = |d: &Diagnostic| {
            let r = d.range;
            (
                r.is_none(),
                r.and_then(|r| program.file_table.path(r.file)),
                r.map_or(u32::MAX, |r| r.line),
                r.map_or(u32::MAX, |r| r.col),
            )
        };
        diags.sort_by(|a, b| {
            (
                &a.rule,
                a.severity() as u8,
                loc(a),
                &a.message,
                &a.var,
                a.hook_label,
            )
                .cmp(&(
                    &b.rule,
                    b.severity() as u8,
                    loc(b),
                    &b.message,
                    &b.var,
                    b.hook_label,
                ))
        });
        safe_checks.sort_by(|a, b| a.rule.cmp(b.rule));

        ComponentFindings {
            diagnostics: diags,
            safe_checks,
        }
    }

    fn clamped(&self, d: Diagnostic) -> Diagnostic {
        match self
            .overrides
            .entries
            .get(d.rule.as_ref())
            .and_then(|e| e.ceiling)
        {
            Some(ceiling) => d.clamp(ceiling),
            None => d,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub rule emitting one Warning under a fixed diagnostic name.
    struct Stub {
        id: &'static str,
    }
    impl Rule for Stub {
        fn name(&self) -> &str {
            self.id
        }
        fn check(&self, _ctx: &RuleCtx) -> Vec<Diagnostic> {
            vec![Diagnostic::warn(
                std::borrow::Cow::Owned(self.id.to_string()),
                "stub finding",
            )]
        }
    }

    fn stub_doc(name: &str) -> RuleDoc {
        RuleDoc::new(name.to_string(), "s", "e", "x", "f")
    }

    #[test]
    fn natives_match_all_rules_order() {
        let reg = RuleRegistry::natives();
        let names: Vec<String> = reg.rules.iter().map(|r| r.name().to_string()).collect();
        let expected: Vec<String> = all_rules().iter().map(|r| r.name().to_string()).collect();
        assert_eq!(names, expected);
        assert_eq!(reg.docs.len(), RULE_DOCS.len());
    }

    #[test]
    fn register_rejects_bare_names() {
        let mut reg = RuleRegistry::natives();
        let err = reg
            .register(Box::new(Stub { id: "custom" }), stub_doc("custom"))
            .unwrap_err();
        assert_eq!(err, RegistryError::BareDynamicName("custom".into()));
    }

    #[test]
    fn register_rejects_duplicates() {
        let mut reg = RuleRegistry::natives();
        reg.register(Box::new(Stub { id: "team/x" }), stub_doc("team/x"))
            .unwrap();
        let err = reg
            .register(Box::new(Stub { id: "team/x" }), stub_doc("team/x"))
            .unwrap_err();
        assert_eq!(err, RegistryError::DuplicateName("team/x".into()));
    }

    #[test]
    fn overrides_reject_unknown_keys() {
        let mut reg = RuleRegistry::natives();
        let mut o = RuleOverrides::default();
        o.entries.insert("no-such-rule".into(), OverrideEntry::default());
        assert_eq!(
            reg.set_overrides(o).unwrap_err(),
            RegistryError::UnknownRule("no-such-rule".into())
        );
    }

    #[test]
    fn overrides_reject_options_on_native() {
        let mut reg = RuleRegistry::natives();
        let mut o = RuleOverrides::default();
        let mut opts = serde_json::Map::new();
        opts.insert("k".into(), serde_json::Value::Bool(true));
        o.entries.insert(
            "missing-deps".into(),
            OverrideEntry {
                options: opts,
                ..Default::default()
            },
        );
        assert_eq!(
            reg.set_overrides(o).unwrap_err(),
            RegistryError::OptionsOnNative("missing-deps".into())
        );
    }

    #[test]
    fn overrides_reject_options_on_diagnostic_only_name() {
        let mut reg = RuleRegistry::natives();
        let mut o = RuleOverrides::default();
        let mut opts = serde_json::Map::new();
        opts.insert("k".into(), serde_json::Value::Bool(true));
        o.entries.insert(
            "cross-setter-in-render".into(),
            OverrideEntry {
                options: opts,
                ..Default::default()
            },
        );
        assert_eq!(
            reg.set_overrides(o).unwrap_err(),
            RegistryError::OptionsOnDiagnosticOnly("cross-setter-in-render".into())
        );
    }

    #[test]
    fn overrides_accept_severity_on_diagnostic_only_name() {
        let mut reg = RuleRegistry::natives();
        let mut o = RuleOverrides::default();
        o.entries.insert(
            "cross-setter-in-render".into(),
            OverrideEntry {
                ceiling: Some(Severity::Warning),
                ..Default::default()
            },
        );
        assert!(reg.set_overrides(o).is_ok());
    }

    // ── check_component behavior, on a minimal analyzed component ────────────

    fn one_component() -> (ProgramAnalysisResult, Symbol) {
        let cfg = crate::test_support::single_block_cfg(vec![]);
        let result = crate::test_support::analysis_result(cfg);
        (crate::test_support::prog("C", result), "C".to_string())
    }

    fn registry_with_stub() -> RuleRegistry {
        let mut reg = RuleRegistry::natives();
        reg.register(Box::new(Stub { id: "test/stub" }), stub_doc("test/stub"))
            .unwrap();
        reg
    }

    #[test]
    fn stub_rule_fires_through_the_registry() {
        let (prog, name) = one_component();
        let findings = registry_with_stub().check_component(&prog, &name);
        assert!(findings.diagnostics.iter().any(|d| d.rule == "test/stub"));
    }

    #[test]
    fn off_drops_the_finding() {
        let (prog, name) = one_component();
        let mut reg = registry_with_stub();
        let mut o = RuleOverrides::default();
        o.entries.insert(
            "test/stub".into(),
            OverrideEntry {
                off: true,
                ..Default::default()
            },
        );
        reg.set_overrides(o).unwrap();
        let findings = reg.check_component(&prog, &name);
        assert!(!findings.diagnostics.iter().any(|d| d.rule == "test/stub"));
    }

    #[test]
    fn allow_restricts_visibility() {
        let (prog, name) = one_component();
        let mut reg = registry_with_stub();
        let mut o = RuleOverrides::default();
        o.allow = Some(BTreeSet::from(["missing-deps".to_string()]));
        reg.set_overrides(o).unwrap();
        let findings = reg.check_component(&prog, &name);
        assert!(!findings.diagnostics.iter().any(|d| d.rule == "test/stub"));
    }

    #[test]
    fn off_beats_allow() {
        // `off` always drops, even when the name is on the allowlist — the
        // frontend decides which composition (config off + --rule) is soft.
        let (prog, name) = one_component();
        let mut reg = registry_with_stub();
        let mut o = RuleOverrides::default();
        o.entries.insert(
            "test/stub".into(),
            OverrideEntry {
                off: true,
                ..Default::default()
            },
        );
        o.allow = Some(BTreeSet::from(["test/stub".to_string()]));
        reg.set_overrides(o).unwrap();
        let findings = reg.check_component(&prog, &name);
        assert!(!findings.diagnostics.iter().any(|d| d.rule == "test/stub"));
    }

    #[test]
    fn ceiling_clamps_the_finding() {
        let (prog, name) = one_component();
        let mut reg = registry_with_stub();
        let mut o = RuleOverrides::default();
        o.entries.insert(
            "test/stub".into(),
            OverrideEntry {
                ceiling: Some(Severity::Info),
                ..Default::default()
            },
        );
        reg.set_overrides(o).unwrap();
        let findings = reg.check_component(&prog, &name);
        let d = findings
            .diagnostics
            .iter()
            .find(|d| d.rule == "test/stub")
            .unwrap();
        assert_eq!(d.severity(), Severity::Info);
    }

    #[test]
    fn ceiling_cannot_upgrade() {
        // The soundness case: pinning a Warning-polarity finding to "error"
        // is a structural no-op.
        let (prog, name) = one_component();
        let mut reg = registry_with_stub();
        let mut o = RuleOverrides::default();
        o.entries.insert(
            "test/stub".into(),
            OverrideEntry {
                ceiling: Some(Severity::Error),
                ..Default::default()
            },
        );
        reg.set_overrides(o).unwrap();
        let findings = reg.check_component(&prog, &name);
        let d = findings
            .diagnostics
            .iter()
            .find(|d| d.rule == "test/stub")
            .unwrap();
        assert_eq!(d.severity(), Severity::Warning);
    }

    #[test]
    fn off_on_one_diagnostic_name_keeps_the_other() {
        // A rule emitting two diagnostic names must not have its second name
        // swallowed by an `off` on the first (the FN guard): rules always
        // run; findings are dropped by name.
        struct TwoNames;
        impl Rule for TwoNames {
            fn name(&self) -> &str {
                "test/two"
            }
            fn check(&self, _ctx: &RuleCtx) -> Vec<Diagnostic> {
                vec![
                    Diagnostic::warn("test/two", "main"),
                    Diagnostic::warn("test/two-cross", "secondary"),
                ]
            }
        }
        let (prog, name) = one_component();
        let mut reg = RuleRegistry::natives();
        reg.register(Box::new(TwoNames), stub_doc("test/two")).unwrap();
        // The secondary diagnostic name needs a doc for override validation;
        // registry docs are keyed by diagnostic name.
        reg.docs.push(stub_doc("test/two-cross"));
        let mut o = RuleOverrides::default();
        o.entries.insert(
            "test/two".into(),
            OverrideEntry {
                off: true,
                ..Default::default()
            },
        );
        reg.set_overrides(o).unwrap();
        let findings = reg.check_component(&prog, &name);
        assert!(!findings.diagnostics.iter().any(|d| d.rule == "test/two"));
        assert!(
            findings
                .diagnostics
                .iter()
                .any(|d| d.rule == "test/two-cross")
        );
    }
}
