//! `reactant.config.json` — the consumer configuration file (ADR-022 §5).
//!
//! Lives in the library (clap-free) so every frontend — native CLI, WASM
//! host — parses and validates the same way. JSONC-tolerant (the tsconfig
//! precedent: [`crate::project::strip_jsonc`]), loudly validated: a present
//! but broken config is always an error, never silently degraded to defaults
//! (silent config loss would silently drop packs and overrides).
//!
//! Precedence: CLI flags beat config values; the merge lives with the
//! frontend options, not here.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[cfg(feature = "schema-gen")]
use schemars::JsonSchema;

use crate::project::strip_jsonc;
use crate::rules::Severity;

pub const CONFIG_FILE_NAME: &str = "reactant.config.json";

#[derive(Debug, Default, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReactantConfig {
    /// Editor-facing schema URL; not interpreted.
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    /// Rule packs to load: npm package names or relative paths, in order
    /// (ADR-022 §8: pack order is output order). Consumed by the pack loader.
    #[serde(default)]
    pub packs: Vec<String>,
    /// Per-diagnostic overrides: `"off" | "<severity>" | { severity?, options? }`.
    #[serde(default)]
    pub rules: BTreeMap<String, RuleSetting>,

    // ── `check` flag equivalents (CLI takes precedence, ADR-022 §5) ──────────
    #[serde(default)]
    pub entry: Vec<String>,
    #[serde(default)]
    pub all_roots: Option<bool>,
    #[serde(default)]
    pub fail_on: Option<FailOnConfig>,
    #[serde(default)]
    pub project: Option<ProjectConfig>,
    #[serde(default)]
    pub format: Option<FormatConfig>,
    #[serde(default)]
    pub info: Option<bool>,
    #[serde(default)]
    pub show_clean: Option<bool>,
    #[serde(default)]
    pub trace: Option<bool>,
    /// Directory names never walked, matched at any depth. Non-empty replaces
    /// the default policy (`.gitignore` if the tree has one, else
    /// `dist`/`build`/`.next`); `node_modules` is excluded regardless.
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum FailOnConfig {
    Error,
    Warning,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ProjectConfig {
    Auto,
    Vite,
    Next,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum FormatConfig {
    Human,
    Json,
}

/// One `rules` entry, normalized from its three JSON forms:
/// `"off"`, `"<severity>"`, `{ "severity": …, "options": {…} }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleSetting {
    pub off: bool,
    /// Severity ceiling (ADR-022 §3: `pin ⊓ polarity` — downgrade-only).
    pub severity: Option<Severity>,
    pub options: serde_json::Map<String, serde_json::Value>,
}

const LEVELS: &str = r#""off", "error", "warning" or "info""#;

fn level_from_str<E: serde::de::Error>(s: &str) -> Result<RuleSetting, E> {
    let mut setting = RuleSetting::default();
    match s {
        "off" => setting.off = true,
        "error" => setting.severity = Some(Severity::Error),
        "warning" => setting.severity = Some(Severity::Warning),
        "info" => setting.severity = Some(Severity::Info),
        other => {
            return Err(E::custom(format!(
                "unknown rule setting `{other}` — expected {LEVELS}"
            )));
        }
    }
    Ok(setting)
}

// Manual Deserialize: an untagged enum would report "data did not match any
// variant", useless for the loud-validation contract — every message must
// say what was expected and what was found.
impl<'de> Deserialize<'de> for RuleSetting {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = RuleSetting;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(
                    f,
                    "{LEVELS}, or an object {{ \"severity\": …, \"options\": {{…}} }}"
                )
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<RuleSetting, E> {
                level_from_str(s)
            }

            fn visit_map<A>(self, mut map: A) -> Result<RuleSetting, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut setting = RuleSetting::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "severity" => {
                            let s: String = map.next_value()?;
                            setting = RuleSetting {
                                options: std::mem::take(&mut setting.options),
                                ..level_from_str(&s)?
                            };
                        }
                        "options" => {
                            setting.options = map.next_value()?;
                        }
                        other => {
                            return Err(serde::de::Error::custom(format!(
                                "unknown key `{other}` in rule setting — expected \
                                 \"severity\" or \"options\""
                            )));
                        }
                    }
                }
                Ok(setting)
            }
        }
        deserializer.deserialize_any(V)
    }
}

#[cfg(feature = "schema-gen")]
impl JsonSchema for RuleSetting {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RuleSetting".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "\"off\", a severity ceiling, or a detailed form with options",
            "oneOf": [
                { "type": "string", "enum": ["off", "error", "warning", "info"] },
                {
                    "type": "object",
                    "properties": {
                        "severity": { "type": "string", "enum": ["off", "error", "warning", "info"] },
                        "options": { "type": "object" }
                    },
                    "additionalProperties": false
                }
            ]
        })
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, std::io::Error),
    Invalid(PathBuf, String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(p, e) => write!(f, "cannot read {}: {e}", p.display()),
            ConfigError::Invalid(p, msg) => write!(f, "invalid {}: {msg}", p.display()),
        }
    }
}

/// Parse config text (JSONC-tolerant), validating the shape. `origin` names
/// the source in errors. Every failure is `Err`.
pub fn parse(src: &str, origin: &Path) -> Result<ReactantConfig, ConfigError> {
    serde_json::from_str(&strip_jsonc(src))
        .map_err(|e| ConfigError::Invalid(origin.to_path_buf(), e.to_string()))
}

/// Read, strip JSONC, parse, validate the shape. Every failure is `Err`.
pub fn load(path: &Path) -> Result<ReactantConfig, ConfigError> {
    let src = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.to_path_buf(), e))?;
    parse(&src, path)
}

/// The `check` flags that have config equivalents, in host-neutral form —
/// each frontend maps its argv into this, merges, and maps out to
/// [`crate::driver::CheckOptions`]. One precedence mechanism (ADR-022 §5:
/// flags beat config), two hosts.
#[derive(Debug, Default)]
pub struct CheckArgsPartial {
    pub info: bool,
    pub show_clean: bool,
    pub trace: bool,
    pub verbose: bool,
    pub all_roots: bool,
    pub entry: Vec<String>,
    pub exclude_dirs: Vec<String>,
    pub format: Option<FormatConfig>,
    pub fail_on: Option<FailOnConfig>,
    pub project: Option<ProjectConfig>,
}

impl CheckArgsPartial {
    /// Config values fill only the holes the flags left. Boolean flags are
    /// turn-on-only — no `--no-info` exists to detect.
    pub fn merge(&mut self, cfg: &ReactantConfig) {
        if self.entry.is_empty() {
            self.entry = cfg.entry.clone();
        }
        if self.exclude_dirs.is_empty() {
            self.exclude_dirs = cfg.exclude_dirs.clone();
        }
        self.all_roots |= cfg.all_roots.unwrap_or(false);
        self.info |= cfg.info.unwrap_or(false);
        self.show_clean |= cfg.show_clean.unwrap_or(false);
        self.trace |= cfg.trace.unwrap_or(false);
        self.fail_on = self.fail_on.or(cfg.fail_on);
        self.format = self.format.or(cfg.format);
        self.project = self.project.or(cfg.project);
    }
}

/// Resolve the config `rules` map and the CLI filters into one override set
/// (ADR-022 §5): `--ignore-rule` always denies; an explicit `--rule X`
/// resurrects a config-`"off"` X; severity pins come from config only.
pub fn resolve_overrides(
    cfg: &ReactantConfig,
    rule: &[String],
    ignore_rule: &[String],
) -> crate::rules::RuleOverrides {
    let mut overrides = crate::rules::RuleOverrides::default();
    for (name, setting) in &cfg.rules {
        let entry = overrides.entries.entry(name.clone()).or_default();
        entry.off = setting.off && !rule.contains(name);
        entry.ceiling = setting.severity;
        entry.options = setting.options.clone();
    }
    for name in ignore_rule {
        overrides.entries.entry(name.clone()).or_default().off = true;
    }
    if !rule.is_empty() {
        overrides.allow = Some(rule.iter().cloned().collect());
    }
    overrides
}

/// `<root>/reactant.config.json` when it exists — the same root-marker
/// contract as `vite.config.*`/tsconfig detection: no upward walk, no cwd
/// fallback (an explicit `--config <path>` covers every other layout).
pub fn discover(root: &Path) -> Option<PathBuf> {
    let candidate = root.join(CONFIG_FILE_NAME);
    candidate.is_file().then_some(candidate)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Result<ReactantConfig, String> {
        serde_json::from_str(&strip_jsonc(src)).map_err(|e| e.to_string())
    }

    #[test]
    fn parses_the_adr_example_with_comments() {
        let cfg = parse(
            r#"{
              "$schema": "./node_modules/reactant-analyzer/schemas/reactant-config.schema.json",
              "packs": ["@team/react-rules", "./rules/pack.json"],
              "rules": {
                "infinite-loop": "warning",                  // native downgrade: allowed
                "team/effect-writes-own-dep": { "severity": "error", "options": { "maxDeps": 8 } },
                "missing-deps": "off"                        // subsumes --ignore-rule
              }
            }"#,
        )
        .unwrap();
        assert_eq!(
            cfg.packs,
            vec![
                "@team/react-rules".to_string(),
                "./rules/pack.json".to_string()
            ]
        );
        assert_eq!(
            cfg.rules["infinite-loop"],
            RuleSetting {
                severity: Some(Severity::Warning),
                ..Default::default()
            }
        );
        assert!(cfg.rules["missing-deps"].off);
        let detailed = &cfg.rules["team/effect-writes-own-dep"];
        assert_eq!(detailed.severity, Some(Severity::Error));
        assert_eq!(
            detailed.options.get("maxDeps"),
            Some(&serde_json::Value::from(8))
        );
    }

    #[test]
    fn camel_case_flag_equivalents() {
        let cfg = parse(
            r#"{ "failOn": "never", "allRoots": true, "showClean": true,
                 "project": "vite", "format": "json", "entry": ["App"] }"#,
        )
        .unwrap();
        assert_eq!(cfg.fail_on, Some(FailOnConfig::Never));
        assert_eq!(cfg.all_roots, Some(true));
        assert_eq!(cfg.show_clean, Some(true));
        assert_eq!(cfg.project, Some(ProjectConfig::Vite));
        assert_eq!(cfg.format, Some(FormatConfig::Json));
        assert_eq!(cfg.entry, vec!["App".to_string()]);
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        let err = parse(r#"{ "rulez": {} }"#).unwrap_err();
        assert!(err.contains("rulez"), "{err}");
    }

    #[test]
    fn misspelled_severity_names_the_valid_ones() {
        let err = parse(r#"{ "rules": { "missing-deps": "warn" } }"#).unwrap_err();
        assert!(err.contains("unknown rule setting `warn`"), "{err}");
        assert!(err.contains(r#""warning""#), "{err}");
    }

    #[test]
    fn unknown_rule_setting_key_is_rejected() {
        let err = parse(r#"{ "rules": { "missing-deps": { "level": "error" } } }"#).unwrap_err();
        assert!(err.contains("unknown key `level`"), "{err}");
    }

    #[test]
    fn detailed_form_severity_off_and_options() {
        let cfg = parse(r#"{ "rules": { "x/y": { "severity": "off" } } }"#).unwrap();
        assert!(cfg.rules["x/y"].off);
    }

    #[test]
    fn discover_finds_only_at_root() {
        let dir =
            std::env::temp_dir().join(format!("reactant-config-discover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(discover(&dir), None);
        let path = dir.join(CONFIG_FILE_NAME);
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(discover(&dir), Some(path));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn broken_json_is_an_error_never_defaults() {
        let dir =
            std::env::temp_dir().join(format!("reactant-config-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE_NAME);
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(load(&path), Err(ConfigError::Invalid(..))));
        assert!(matches!(
            load(&dir.join("absent.json")),
            Err(ConfigError::Io(..))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
