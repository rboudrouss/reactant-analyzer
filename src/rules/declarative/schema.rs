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
    /// Non-hook call sites in the render body (#126, ADR-036) — the same
    /// relation the `calls` edge exposes, anchored where there is no hook to
    /// hang an edge on. `router.push(…)` during render lives here.
    ///
    /// A `name` guard is mandatory, for the same reason it is on the edge.
    RenderCalls,
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
    /// Every prop of every element the render body builds (#71 step 2, #125).
    /// Edge-less; `name` is the element, `prop` the prop's name, `identity`
    /// the value's identity verdict, `kind` whether the element is a component
    /// application or a host element.
    ///
    /// `elements` selects which kinds are enumerated and defaults to
    /// `component` — what the relation has always meant, and the only place a
    /// prop is compared by `Object.is` across a memo boundary. `host` binds
    /// `<input ref={r} value={v}/>` instead, and `any` binds both. It is an
    /// anchor option rather than a widening-by-guard because it changes which
    /// rows exist, and a shipped pack must keep binding the rows it always did
    /// (ADR-027 §2, the rule #107 followed).
    JsxProps {
        #[serde(default)]
        elements: Option<ElementsName>,
    },
    /// One render-loop cycle of the program's churn graph, seen from the
    /// effect of THIS component that carries one of its edges (#108,
    /// ADR-029). Edge-less: `cycle` renders the loop as `a → b → a`, and the
    /// `cycle` guard filters on the two exact folds the graph already
    /// computed — whether the loop spans components, and whether every step
    /// is a must-step.
    ///
    /// A row's identity is the carrying edge's write site (ADR-024), so a
    /// cycle whose carrying edge has no span produces none. No `must_*` guard
    /// accepts this sort, so an Error is not reachable through the anchor.
    ChurnCycles,
    /// One `useContext` call site whose ancestry the analysis could complete
    /// (#115, ADR-032). `name` is the local name the call reads the context by;
    /// the `provider` guard says whether any component that may render this one
    /// provides that same cell.
    ///
    /// A row exists only when every ancestor chain is complete — inter-analysed,
    /// non-recursive, and not mentioned by any component phase 1 never reached.
    /// The verdict is an ABSENCE, and an absence is only as good as the paths
    /// you can see, so an incomplete closure produces no row rather than a
    /// confident one. Edge-less; no `must_*` accepts the sort, so Error is
    /// unreachable.
    ContextConsumers,
    /// One callback registration in an effect body (#111/#116, ADR-034): a
    /// call that hands a callback to something outliving the effect. `name` is
    /// the registrar (`setInterval`, `addEventListener`, `subscribe`, …),
    /// `firing` whether it re-fires unboundedly, `listener` the site-identity
    /// verdict of the callback.
    ///
    /// The relation is a **may**-registration: a match on the registrar name
    /// table, never a proof that the callee is the host primitive. That is
    /// wontfix #42's accepted-FP decision, now extended to the public
    /// vocabulary — so the polarity is capped at may/Warning, no `must_*`
    /// binds this sort, and Error is unreachable through the anchor.
    /// Edge-less; the `teardown` guard carries the pairing fact.
    ///
    /// Optional `firing` narrows the enumeration the way `kind` does for
    /// `hook_calls`: `repeating` selects the registrars that keep firing until
    /// torn down, which is the only class where a missing teardown
    /// accumulates. Absent = every row.
    Registrations {
        #[serde(default)]
        firing: Option<FiringName>,
    },
}

/// Which elements a `jsx_props` anchor enumerates (#125).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ElementsName {
    /// Resolved component applications only — the default.
    Component,
    /// Host elements only (`<div/>`, `<input/>`).
    Host,
    Any,
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
    /// Writers of a state-hook anchor's slot (ADR-027 §1, ADR-028 §1): one row
    /// per **call site**, spliced wrappers' setter params included. Two
    /// `setCount(…)` calls in one body are two rows; one write a local helper
    /// contributes is one row however many times the helper is called.
    /// `{w.region}` is the lexical body — exact; `{w.phase}` is a MAY verdict,
    /// `unknown` = may run in any phase.
    Writers,
    /// Non-hook call sites in the anchor's body CFG (#126, ADR-036): one row
    /// per call the setter walk passes, carrying the callee `name`, the
    /// `receiver` root of a member call, and the `phase` it runs in — the same
    /// lattice the writer rows use, so a call in a `.then`, after an `await`,
    /// or in the effect's returned cleanup is distinguishable from one in the
    /// body.
    ///
    /// Unbounded, unlike every other relation: it enumerates *every* call.
    /// A `name` guard on the bound row is therefore **mandatory** — a rule
    /// that fires on "some call" fires on all of them.
    Calls,
    /// Readers of a state-hook anchor's slot (#127, ADR-037): one row per read
    /// site, over the same regions the `writers` edge enumerates. `{r.region}`
    /// is the lexical body — exact; `{r.phase}` is the same MAY verdict, so a
    /// read in a `.then` continuation or a cleanup is distinguishable from one
    /// in the render body.
    ///
    /// A read the walk never entered — inside a closure nothing calls, past the
    /// depth cap — contributes no row, so the ABSENCE of rows is not a proof
    /// that the slot is unread. `none` over this edge reads as "no read the
    /// analysis could see", and over-reports rather than losing a finding.
    Reads,
    /// Prop seeds of a state-hook anchor's slot (#106, ADR-031): one row per
    /// prop path the `useState` initializer reads. `{s.path}` is the path as
    /// written; the `seed_sync` guard reads whether anything visibly re-syncs
    /// the slot when that prop moves.
    ///
    /// A slot whose initializer reads no prop has no rows, so a rule on this
    /// edge is silent on it by construction — that is knowledge, not a filter.
    Seeds,
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
    /// Receiver filter on a `calls` row: the root binding a member call was
    /// made on (`socket` in `socket.join(r)`). The other half of a callee —
    /// `name` says which method ran, this says whose. A bare call has no
    /// receiver and fails the guard, positive-only like every name filter.
    /// Exactly one of `one_of`/`prefix`.
    Receiver {
        of: String,
        #[serde(default)]
        one_of: Option<PVal<Vec<String>>>,
        #[serde(default)]
        prefix: Option<PVal<String>>,
    },
    /// Execution phase of a `calls` (#126) or `reads` (#127) row — a total
    /// mirror of [`PhaseName`], ⊤ included, so a rule that wants "provably
    /// deferred" says `is: ["deferred"]` and one that will accept ⊤ names it.
    ///
    /// Positive-only, because the fact is may-typed: `unknown` means the call
    /// may run in any phase, and a negated form would let a rule suppress a
    /// finding on a ⊤ row.
    Phase {
        of: String,
        is: PVal<Vec<PhaseName>>,
    },
    /// Prop-name filter on a `jsx_props` row (#125): `value`, `key`,
    /// `children`, `onChange`. The relation always carried the field; without
    /// a guard on it a rule could not skip `children` (fresh on every wrapper)
    /// nor scope itself to one prop. Exactly one of `one_of`/`prefix`.
    Prop {
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
    /// Whether a provider of a `context_consumers` row's context sits on a
    /// path that reaches it (#115).
    ///
    /// MAY-typed and positive-only. `none-on-analyzed-paths` is named for what
    /// it is: what the completed paths showed, never a proof that no provider
    /// exists — an unanalyzed mounting shell above a root, an inline-arrow
    /// provider (#30), or a value-position component reference (#63) all land
    /// there. No `must_*` binds the sort, so a rule keyed on it is capped at
    /// Warning by construction.
    Provider {
        of: String,
        is: PVal<Vec<ProviderName>>,
    },
    /// Whether anything visibly takes a `registrations` row back (#111): the
    /// effect's cleanup calls the registrar's teardown holding the **same
    /// listener binding**.
    ///
    /// MAY-typed in one direction, so positive-only, exactly like `seed_sync`.
    /// `paired` is a claim made from a teardown that was read; `none-seen` is
    /// the absence of one, and an unreadable cleanup or a listener that is not
    /// a resolvable name both land there rather than being refuted. Matching
    /// the teardown *name* alone would certify the very shape a
    /// fresh-listener rule exists to catch, so the binding is the fact.
    Teardown {
        of: String,
        is: PVal<Vec<TeardownName>>,
    },
    /// Does an `effect` anchor register a callback that outlives it (#111)?
    ///
    /// Existential MAY over the effect's registration rows, filtered by firing
    /// class: `repeating` for the ones that keep firing until torn down
    /// (`setInterval`, `addEventListener`, `subscribe`, `on`, `addListener`),
    /// `once` for a timeout, a rAF or a promise continuation. Positive-only —
    /// the relation is a name-table match, so "registers nothing" is not a
    /// promise the engine keeps and there is no negated form to assert it
    /// with.
    Registers {
        of: String,
        firing: PVal<Vec<FiringName>>,
    },
    /// Whether anything visibly re-syncs a `seeds` row's slot when that prop
    /// moves (#106): a render-time write, or an effect whose declared deps
    /// cover the seed path (or that declares none, so it re-runs every
    /// render).
    ///
    /// MAY-typed in one direction, and the guard is positive-only for exactly
    /// that reason: `synced` is a claim the relation makes from a write it
    /// saw, `none-seen` is the absence of one. A setter the component handed
    /// out could be called from anywhere, so "no sync exists" is not a promise
    /// the engine keeps — which is why the name is `none-seen` and not
    /// `unsynced`.
    ///
    /// Structurally may-typed: no `must_*` guard binds a seed row, so a rule
    /// keyed on this cannot reach Error. `must_frozen_seed` stays native and
    /// is deliberately not exposed — it certifies a motion proof this relation
    /// does not carry.
    SeedSync {
        of: String,
        is: PVal<Vec<SeedSyncName>>,
    },
    /// Who owns the state slot a `render_setter_calls` row writes (#107):
    /// `local` — the anchored component's own `useState` — or `foreign`, a
    /// `ComponentSetter`-valued prop the top-down inter-component pass placed
    /// in this component's environment.
    ///
    /// **Naming ownership is what widens the enumeration.** Without this
    /// guard the sort binds local rows only, exactly as it did before foreign
    /// rows existed: changing what a shipped sort enumerates changes which
    /// findings a shipped pack fires (ADR-027 §2).
    ///
    /// Two-valued and total — every enumerated row is one or the other by
    /// construction — but the *attribution* is may-typed: it is the same
    /// per-block existential the native `setter-in-render` rule consumes, so a
    /// variable that holds the parent's setter on one path and something else
    /// on another still produces a row. Over-reporting, never a miss.
    SlotOwnership {
        of: String,
        is: PVal<Vec<OwnershipName>>,
    },
    /// Shape filter on a `churn_cycles` row (#108): whether the loop spans
    /// more than one component, and whether every one of its steps is a
    /// must-step. At least one of `cross_component` / `all_must`; the given
    /// fields are conjoined.
    ///
    /// Both are **exact** booleans — folds of the node table and the edge
    /// strengths the graph already computed — so unlike the may-typed verdict
    /// guards this one has a meaningful negative and takes plain booleans
    /// rather than a ⊤-bearing name list. What is may-typed is the graph
    /// itself: a cycle it never saw yields no row at all, which is the
    /// missing-findings direction.
    Cycle {
        of: String,
        #[serde(default)]
        cross_component: Option<PVal<bool>>,
        #[serde(default)]
        all_must: Option<PVal<bool>>,
    },
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
    /// Negated existential over an edge of the anchor (#126): passes when
    /// **no** row satisfies the nested guards.
    ///
    /// The one thing the guard language could not say, and what the wish-list
    /// kept asking for: "acquires a resource and releases none", "has a `value`
    /// prop and no `onChange`", "subscribes and never reads the current value".
    /// A `forEach` is the existential; this is its negation, and neither can be
    /// written as the other.
    ///
    /// **The unsound direction is the safe one here.** Every relation it can
    /// quantify over may under-enumerate — a depth-capped walk, a callee it
    /// could not resolve — and a missing row makes `none` pass, so the rule
    /// fires where it should not. That is a false positive, which this project
    /// accepts; the direction it never takes is losing a finding.
    ///
    /// Never mints a proof, exactly like `every`: a `must_*` anywhere in a rule
    /// that uses it is rejected at load time.
    #[serde(rename = "none")]
    NoneOf {
        /// `anchor.<edge>` — the same spelling `count` and `every` use, and
        /// typed by the same table `forEach` navigation reads.
        of: String,
        /// Name the row binds under inside `guards`, owned by the quantifier
        /// for its own subtree and not visible in the message.
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

/// Total mirror of the provider verdict (#115). Two-valued: the second name
/// reads as an absence of evidence, never as a proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ProviderName {
    /// Some component that may render this one provides the cell.
    ProviderSeen,
    /// None did, on the paths the analysis could complete.
    NoneOnAnalyzedPaths,
}

/// Total mirror of the registration↔teardown pairing verdict (#111).
/// Two-valued in the schema although the engine's fact is three-valued: the
/// unresolvable case folds into `none-seen`, which reads as an absence of
/// evidence and never as a proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum TeardownName {
    /// The effect's cleanup calls the registrar's teardown holding the same
    /// listener binding.
    Paired,
    /// It does not, or the walk could not read the cleanup. Not a proof that
    /// no teardown exists.
    NoneSeen,
}

/// How a registration re-fires (#111). Two-valued and total: every row in the
/// registrar table is one or the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum FiringName {
    /// Keeps firing until torn down: a timer interval, an event listener, a
    /// subscription.
    Repeating,
    /// Fires once, shortly after registration: a timeout, a rAF, a promise
    /// continuation.
    Once,
}

/// Total mirror of the seed-sync verdict (#106). Two-valued: `none-seen` is
/// the may side and reads as an absence of evidence, never as a proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SeedSyncName {
    /// A write that re-runs when this prop moves was seen.
    Synced,
    /// None was seen. Not a proof that none exists.
    NoneSeen,
}

/// Who owns the slot a render-setter row writes (#107). Two-valued and total:
/// a row's owner is resolved or the row does not exist, so there is no ⊤.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum OwnershipName {
    /// The anchored component's own state slot.
    Local,
    /// Another component's slot, reached through a `ComponentSetter` prop.
    Foreign,
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
