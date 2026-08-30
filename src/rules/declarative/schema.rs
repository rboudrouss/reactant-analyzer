//! Serde model of `pack.json` (ADR-022 §5) — the single source of truth for
//! the published JSON schema (schemars derives, behind `schema-gen`).
//!
//! Deserialization strategy: everything is derived, including the
//! internally-tagged `Guard`/`Anchor` enums. serde cannot enforce
//! `deny_unknown_fields` on internally-tagged enums, so unknown-key checks
//! for guards and anchors are done by the validator against the raw JSON
//! value (`validate::check_unknown_keys`) — same loudness, exact paths.

use std::collections::BTreeMap;

use serde::Deserialize;

#[cfg(feature = "schema-gen")]
use schemars::JsonSchema;

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PackFile {
    /// Editor-facing schema URL; not interpreted. Accepted for the same reason
    /// `reactant.config.json` accepts it — a published schema is only useful if
    /// the file is allowed to point at it.
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    /// Format version; only `1` exists.
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Pack name: the namespace of every rule id (`<name>/<rule>`).
    pub name: String,
    pub rules: Vec<RuleDef>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    /// Bare rule id (no `/`); addressed as `<pack>/<id>`.
    pub id: String,
    pub docs: RuleDocs,
    /// Desired severity ceiling (a pin, ADR-022 §3): the effective severity
    /// of each finding is `pin ⊓ polarity`, evaluated at emission.
    pub severity: SeverityPin,
    /// Declared parameters, referenced as `{"$param": "<name>"}` in leaf
    /// constant positions (ADR-022 §4).
    #[serde(default)]
    pub params: BTreeMap<String, ParamDecl>,
    pub anchor: Anchor,
    #[serde(rename = "forEach", default)]
    pub for_each: Option<ForEach>,
    #[serde(default)]
    pub guards: Vec<Guard>,
    /// Message template interpolating navigated entities (`{setter.slot}`)
    /// and params (`{param.maxDeps}`); `{{`/`}}` escape braces.
    pub message: String,
}

/// Mandatory docs (ADR-022 §5): a custom rule without an explanation is
/// exactly the diagnostic a team learns to ignore.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RuleDocs {
    /// One line — what the rule detects (`reactant rules`).
    pub description: String,
    /// Why it matters (`reactant explain`).
    pub why: String,
    /// How to fix it.
    pub fix: String,
    /// Optional minimal buggy snippet.
    #[serde(default)]
    pub example: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum SeverityPin {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ParamDecl {
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub default: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
pub enum ParamType {
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "string")]
    String,
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "string[]")]
    StringList,
}

/// The anchor: a relation the engine has already resolved (ADR-022 §1) —
/// never a syntax pattern.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(tag = "relation", rename_all = "snake_case")]
pub enum Anchor {
    /// One row of the `hook_calls` table, optionally kind-filtered.
    HookCalls {
        #[serde(default)]
        kind: Option<HookKindFilter>,
    },
    /// Alias-resolved setter calls in the render body.
    RenderSetterCalls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum HookKindFilter {
    State,
    Effect,
    Memo,
    Callback,
    Ref,
    Custom,
    Handler,
}

/// Typed navigation from the anchor (ADR-022 §2): at most one edge, one
/// binding — no joins.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ForEach {
    pub edge: EdgeName,
    #[serde(rename = "as")]
    pub bind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EdgeName {
    /// Declared deps-array entries of an effect/memo/callback anchor.
    Deps,
    /// Alias-resolved setter calls in the anchor's body CFG.
    BodySetterCalls,
    /// Call-site arguments of a custom-hook anchor (ADR-023 §3). Admits the
    /// `returns` guard and NOT `stability`: an argument is evaluated at the
    /// call, so reading the render-exit stability there is the program-point
    /// error ADR-023 §2 refuses.
    Args,
}

/// A guard: a predicate over an engine verdict. `must_*` guards certify
/// (attach the `Certified` proof on `All`); the others filter. The `must_`
/// prefix makes polarity visible in the JSON — the §3 load-time warning is
/// a prefix scan.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Guard {
    /// Stability verdict of a deps entry. Exactly one of `is`/`not`; the
    /// verdict names mirror `StabilityVerdict` totally — ⊤ (`unknown`) can
    /// be matched but never silently dropped.
    Stability {
        of: String,
        #[serde(default)]
        is: Option<PVal<Vec<StabilityName>>>,
        #[serde(default)]
        not: Option<PVal<Vec<StabilityName>>>,
    },
    /// Returns-verdict of a call-site argument (ADR-023 §3): what the
    /// function-valued argument *returns* — the identity question, so a store
    /// selector returning a fresh reference is distinguishable from one
    /// returning a value-compared primitive. Exactly one of `is`/`not`; the
    /// names mirror `ReturnsVerdict` totally — ⊤ (`unknown`) is matchable,
    /// never dropped.
    Returns {
        of: String,
        #[serde(default)]
        is: Option<PVal<Vec<ReturnsName>>>,
        #[serde(default)]
        not: Option<PVal<Vec<ReturnsName>>>,
    },
    /// Provenance filter on a hook-call row (ADR-023 step 1): the hook the
    /// call's identity resolved to (`useLayoutEffect` even when reached
    /// through an alias) and whether the call is written in the component
    /// (`direct: true`) or reached through an inlined wrapper hook. This is
    /// what lets "never call `useLayoutEffect` directly, use the SSR-safe
    /// wrapper" stay silent on conformant consumers of the wrapper. At least
    /// one of `hook`/`direct`; a row with no provenance fails (positive-only).
    Origin {
        of: String,
        #[serde(default)]
        hook: Option<PVal<Vec<String>>>,
        #[serde(default)]
        direct: Option<PVal<bool>>,
    },
    /// The setter's slot appears in the anchor's declared deps.
    InDeps {
        of: String,
        #[serde(default)]
        negate: bool,
    },
    /// Name filter on a resolved entity (ADR-022 §1: names are filters on
    /// resolved entities, never text patterns over call syntax). Exactly one
    /// of `one_of`/`prefix`.
    Name {
        of: String,
        #[serde(default)]
        one_of: Option<PVal<Vec<String>>>,
        #[serde(default)]
        prefix: Option<PVal<String>>,
    },
    /// Import-specifier filter on a custom hook row — the package it was
    /// imported from (`@chakra-ui/react`), which is how a team bans a
    /// dependency rather than a local name. Never the resolved path: that is
    /// absolute, so matching it would tie a pack to one checkout. A hook with
    /// no specifier (defined locally, or imported relatively) does not match.
    /// Exactly one of `one_of`/`prefix`.
    Source {
        of: String,
        #[serde(default)]
        one_of: Option<PVal<Vec<String>>>,
        #[serde(default)]
        prefix: Option<PVal<String>>,
    },
    /// Cardinality of `anchor.<edge>` (only `anchor.deps` in v1). Exactly
    /// one comparator.
    Count {
        of: String,
        #[serde(default)]
        more_than: Option<PVal<u64>>,
        #[serde(default)]
        less_than: Option<PVal<u64>>,
        #[serde(default)]
        equals: Option<PVal<u64>>,
    },
    /// Whether the anchor declares a deps array at all.
    DepsDeclared { of: String, eq: PVal<bool> },
    MustSetterOnAllPaths {
        of: String,
        #[serde(rename = "else", default)]
        r#else: ElseBehavior,
    },
    MustDominatesAllExits {
        of: String,
        #[serde(rename = "else", default)]
        r#else: ElseBehavior,
    },
    MustInitCallsSetter {
        of: String,
        #[serde(rename = "else", default)]
        r#else: ElseBehavior,
    },
    MustHookIsConditional {
        of: String,
        #[serde(rename = "else", default)]
        r#else: ElseBehavior,
    },
    /// Disjunction: the candidate passes when **any** listed guard passes.
    /// The guard list of a rule is a conjunction, so this is the only way to
    /// write "X or Y" without duplicating a rule and its docs.
    ///
    /// Universal quantification over a `forEach` edge is a different question
    /// and stays refused (ADR-023 §4): this composes guards, it does not fold
    /// over elements.
    AnyOf { guards: Vec<Guard> },
}

/// What happens to a finding whose must-guard did not certify: `keep` (the
/// default — it survives as a Warning-ceiling finding, ADR-022 §3's free
/// stratification) or `drop` (explicit opt-in for qualification-style rules).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ElseBehavior {
    #[default]
    Keep,
    Drop,
}

/// Total mirror of `StabilityVerdict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StabilityName {
    Stable,
    Versioned,
    PerRender,
    Unknown,
}

/// Total mirror of `ReturnsVerdict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ReturnsName {
    Stable,
    FreshReference,
    Unknown,
}

/// Value-or-parameter: a leaf constant position that accepts either a JSON
/// value of type `T` or `{"$param": "<name>"}` (ADR-022 §4 — parameters are
/// values, never structure).
#[derive(Debug, Clone, PartialEq)]
pub enum PVal<T> {
    Value(T),
    Param(String),
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for PVal<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(deserializer)?;
        if let serde_json::Value::Object(m) = &v
            && let Some(p) = m.get("$param")
        {
            if m.len() != 1 {
                return Err(D::Error::custom(
                    "a {\"$param\": …} reference takes no other key",
                ));
            }
            return match p {
                serde_json::Value::String(s) => Ok(PVal::Param(s.clone())),
                other => Err(D::Error::custom(format!(
                    "\"$param\" expects a parameter name string, got {other}"
                ))),
            };
        }
        T::deserialize(v).map(PVal::Value).map_err(|e| {
            D::Error::custom(format!(
                "expected a value or {{\"$param\": \"<name>\"}} — {e}"
            ))
        })
    }
}

#[cfg(feature = "schema-gen")]
impl<T: JsonSchema> JsonSchema for PVal<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("PVal_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let inner = generator.subschema_for::<T>();
        schemars::json_schema!({
            "oneOf": [
                inner,
                {
                    "type": "object",
                    "properties": { "$param": { "type": "string" } },
                    "required": ["$param"],
                    "additionalProperties": false
                }
            ]
        })
    }
}
