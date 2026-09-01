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
    /// One `hook_provenance` row: every hook call whose identity the engine
    /// resolved, *surviving* custom-hook inlining (ADR-027 §7 — the #6 fix:
    /// a resolved hook keeps no `hook_calls` row of kind `custom`, so
    /// identity rules anchor here). `name` reads the origin hook's name,
    /// `source` its import specifier; the row carries no kind and no edges.
    HookOrigins,
    /// One proven context-provider element in the render body (#71, ADR-027
    /// §8): `<Ctx.Provider value={…}>` where `Ctx` is a module-level
    /// `createContext` proven by import. `name` reads the context binding,
    /// `identity` the value prop's identity verdict. Render-only by
    /// semantics: an element built inside `useMemo` keeps identity between
    /// recomputations. Edge-less in v1; the any-prop generalisation rides
    /// this same relation later.
    ContextProviders,
    /// Every prop of every resolved component element in the render body
    /// (#71 step 2). Kind-less and edge-less; `name` is the element, `prop`
    /// the prop's name, `identity` the value's identity verdict. Host
    /// elements (`<div/>`) produce no rows — lowering resolved them as
    /// something other than a component application.
    JsxProps,
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
    /// Writers of a state-hook anchor's slot (ADR-027 §1): one row per
    /// (region, alias-resolved setter variable, sync-vs-nested), spliced
    /// wrappers' setter params included. `{w.region}` is the lexical body —
    /// exact; `{w.phase}` is a MAY verdict, `unknown` = may run in any phase.
    Writers,
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
    /// Identity verdict of a `context_providers` row's value (#71). Exactly
    /// one of `is`/`not`; the names mirror `ValueIdentity` totally — ⊤
    /// (`unknown`) is matchable, never dropped.
    Identity {
        of: String,
        #[serde(default)]
        is: Option<PVal<Vec<IdentityName>>>,
        #[serde(default)]
        not: Option<PVal<Vec<IdentityName>>>,
    },
    /// Teardown verdict of an `effect` anchor's own body (#100). Exactly one
    /// of `is`/`not`; the names mirror `CleanupVerdict` totally — ⊤
    /// (`unknown`) is matchable, never dropped. `absent` is the only proven
    /// side (every exit returns nothing), so `is: ["absent"]` cannot fire on
    /// a body whose return could not be classified.
    ///
    /// ADR-023 §1 says the growth path is entities, not guards. This is the
    /// admissible exception the shipped vocabulary already established — §3's
    /// `returns`, then ADR-027's `identity` and `writer_phases`: what §1
    /// refuses is a guard naming a *syntactic shape*, and a total mirror of an
    /// engine verdict read at the anchor's own position names none. There is
    /// no new entity to grow here — the effect row IS the subject, and the
    /// verdict is a property of the body it already carries.
    Cleanup {
        of: String,
        #[serde(default)]
        is: Option<PVal<Vec<CleanupName>>>,
        #[serde(default)]
        not: Option<PVal<Vec<CleanupName>>>,
    },
    /// Write-provenance filter on a `writers` row (ADR-027 §4): whether the
    /// write is caller-authored (`direct`) or reached through named inlined
    /// wrappers (`through` — matched anywhere in the chain, against EXPORTED
    /// names, so aliased imports don't escape). At least one of
    /// `through`/`direct`; a row whose site could not be placed fails both
    /// forms (positive-only).
    Provenance {
        of: String,
        #[serde(default)]
        through: Option<PVal<Vec<String>>>,
        #[serde(default)]
        direct: Option<PVal<bool>>,
    },
    /// MAY existential over the writers of a state-hook anchor's slot
    /// (ADR-027 §1 — the #70 join dissolver): passes when some write of the
    /// slot may run in one of the named phases. A ⊤-phase write (`unknown`)
    /// satisfies every query — suppressing a finding on a may-fact would be
    /// a false negative. Positive-only; there is no negated form.
    WriterPhases {
        of: String,
        includes: PVal<Vec<PhaseName>>,
    },
    /// How a `writers` row's argument 0 classifies (ADR-028 §2). A total
    /// mirror of [`UpdaterName`] — ⊤ (`unknown`) is nameable, so a rule that
    /// wants "not proven functional" says so instead of getting it by
    /// accident. Positive-only: there is no negated form.
    ///
    /// `functional` is claimed only for a proven function literal — inline, or
    /// a variable bound exactly once to one. Everything else folds to
    /// `unknown`, so a rule keyed on it over-reports rather than missing a
    /// write.
    Updater {
        of: String,
        is: PVal<Vec<UpdaterName>>,
    },
    /// Whether a `writers` row's updater body writes to something it does not
    /// own (ADR-028 §2) — a mutation whose receiver roots at a parameter or a
    /// captured name, or a setter call.
    ///
    /// A derived reading of the same `updater` column the [`Guard::Updater`]
    /// guard classifies, never a second column and never a second pass over
    /// the setter argument (ADR-027 §4). A total mirror: `impure` is claimed
    /// only for a proven-rooted site, and everything else — including an
    /// updater the walk could not resolve to a literal — is `unknown`, so ⊤
    /// cannot misfire.
    ///
    /// It is a **presence** fact: the site is in the body CFG or it is not, no
    /// abstract value is read at any program point, so ADR-023 §2's gate does
    /// not apply. Whether a call reaches the site is conditional, which caps
    /// the class at Warning.
    UpdaterBody {
        of: String,
        is: PVal<Vec<ImpureName>>,
    },
    /// A `writers` row whose slot may be written **again** in the same tick:
    /// another sync write of the same slot in the same region is CFG-reachable
    /// from this one, self-reachability through a back edge included (a lone
    /// write inside a loop co-executes with itself).
    ///
    /// The guard carries no value field, and that is the design: the fact is
    /// may-typed in one direction only. Reachability is exact on the CFG, but
    /// the walk that found the writes is depth-capped, so "no other write is
    /// reachable" is not a promise the engine can keep — there is no negated
    /// form to assert it with.
    SameTick { of: String },
    /// Universal quantification over `anchor.deps` (ADR-023 §4, whose stated
    /// gate — "making truncation representable in the IR" — the `exact` bit
    /// discharges): passes when every element satisfies the nested guards.
    ///
    /// **Whether ⊤ satisfies is the body's decision, not the quantifier's.**
    /// The verdict guards name their own ⊤: `is: ["stable"]` means *provably*
    /// stable and a ⊤ element fails it, exactly as it does under a `forEach`;
    /// `is: ["stable", "unknown"]` accepts a list that may conform. Folding
    /// ⊤-satisfies into `every` instead would make the two quantifiers of the
    /// same guard disagree about the same fact, and would fire every
    /// "all deps stable" rule on every effect keyed on a ⊤ prop.
    ///
    /// Positive-only — there is no negated form, and `not every` is just the
    /// existential a `forEach` already writes.
    ///
    /// Quantifying needs a domain. A **written** array supplies one even when
    /// a spread hides part of it — the fold ranges over the elements the
    /// engine can see, and one visible violator refutes ∀ outright. An absent
    /// or unreadable deps argument supplies no element at all, and a claim
    /// about nothing is not a claim the engine may make, so the guard fails
    /// there. A list that is known empty quantifies vacuously true; pair with
    /// `count` when a rule needs at least one element.
    ///
    /// Never mints a proof: a `must_*` guard anywhere inside a rule that uses
    /// `every` is rejected at load time, so an `every`-selected finding cannot
    /// carry Error authority for a row a may-fact put there (ADR-021).
    Every {
        of: String,
        /// Name the element binds under inside `guards`. It is the same slot a
        /// rule-level `forEach` binding uses, which the quantifier owns for
        /// its own subtree — so the outer binding is not visible inside, and
        /// this name is not visible in the message.
        #[serde(rename = "as")]
        r#as: String,
        guards: Vec<Guard>,
    },
    /// Cardinality of `anchor.<edge>` (only `anchor.deps` in v1). Exactly one
    /// comparator.
    ///
    /// An elision keeps the count exact — `[a, , b]` declares three entries,
    /// even though lowering can only show two. A spread leaves a lower bound
    /// instead, and the guard then answers what that bound **refutes**,
    /// passing otherwise: `[a, …, g, ...rest]` provably holds more than five.
    /// Refusing an open-ended list outright would delete findings, which is
    /// the one direction this project does not trade. With no written array at
    /// all there is nothing to count and the guard fails — `deps_declared` is
    /// the guard that asks whether one was passed.
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
    /// Certifies that a `writers` row is caller-authored (ADR-027 §5): the
    /// site sits outside every spliced wrapper region. The proof behind an
    /// Error-pinned "state is only written through our wrapper" policy rule.
    MustDirectWrite {
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

// (`must_direct_write` lives in the Guard enum above; this marker keeps the
// section comment structure intact.)

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

/// Total mirror of the updater-body purity classifier (ADR-028 §2);
/// ⊤ = `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ImpureName {
    /// A mutation rooted at a parameter or a captured name, or a setter call.
    Impure,
    /// ⊤ — nothing provable was found, the updater is not a resolvable
    /// literal, or the receiver could not be rooted.
    Unknown,
}

/// Total mirror of the `writers` updater column (ADR-028 §2); ⊤ = `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum UpdaterName {
    /// Proven a function literal: `set(prev => …)`, or a variable bound
    /// exactly once to one.
    Functional,
    /// ⊤ — a value expression, a call, an argument the walk could not resolve,
    /// or no argument at all.
    Unknown,
}

/// Total mirror of `WriterPhase` (ADR-027 §1); ⊤ = `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum PhaseName {
    Render,
    Effect,
    Memo,
    Callback,
    Handler,
    /// Proved deferred (timer, microtask, promise continuation) — never
    /// inside a React phase.
    Deferred,
    /// An effect's returned cleanup function.
    Cleanup,
    Unknown,
}

/// Total mirror of `ValueIdentity` (#71): what a provider's `value` hands
/// consumers across renders. Two-valued on purpose — `fresh-every-render` is
/// a proven fact, everything else is `unknown` (may side, never actionable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum IdentityName {
    FreshEveryRender,
    Unknown,
}

/// Total mirror of `CleanupVerdict` (#100): what an effect body returns, seen
/// as teardown. `absent` is the claim — every exit returns nothing at all —
/// and `unknown` folds to the may side (there may be a cleanup), so it is
/// matchable but never actionable as an absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum CleanupName {
    Present,
    Absent,
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
