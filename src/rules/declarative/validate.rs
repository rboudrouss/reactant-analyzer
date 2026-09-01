//! Semantic validation of a parsed pack (ADR-022 §5): pack-level identity
//! checks, the entity-sort type system over anchors/edges/guards, `$param`
//! resolution (leaf constants only, §4), template checking — producing the
//! fully-resolved internal IR the executor runs, or a [`PackError`] whose
//! `path` + expected/got message is precise enough to feed an LLM authoring
//! loop (the ADR's stated validator cost).
//!
//! Everything here is *static*: severity is NOT validated (§3 — the pin is a
//! ceiling evaluated per finding at emission); the only severity-related
//! outcome is a [`LoadWarning`] when a pin of `"error"` is statically
//! unreachable (no must-guard), and the rule loads anyway.

use std::collections::BTreeMap;

use crate::rules::docs::rule_doc;

use super::schema::{
    Anchor, CleanupName, EdgeName, ElseBehavior, Guard, HookKindFilter, IdentityName, PVal,
    PackFile, ParamDecl, ParamType, PhaseName, ReturnsName, RuleDef, SeverityPin, StabilityName,
};

/// A pack rejection: `path` is the JSON location (`rules[1].guards[0].of`),
/// `message` says what was expected and what was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackError {
    pub path: String,
    pub message: String,
}

impl PackError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        PackError {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "at `{}`: {}", self.path, self.message)
        }
    }
}

/// A non-fatal load-time notice (the rule loads anyway).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadWarning {
    /// Full rule id (`pack/rule`).
    pub rule: String,
    pub message: String,
}

// ── Entity sorts ──────────────────────────────────────────────────────────────

/// The static type of a bound entity — the universe of the schema's type
/// system. Every edge and guard is typed against these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sort {
    /// A `hook_calls` row; `None` = any kind.
    Hook(Option<HookKindFilter>),
    /// An alias-resolved setter call in the render CFG.
    SetterRender,
    /// An alias-resolved setter call in the anchor's body CFG.
    SetterBody,
    /// One declared deps-array entry.
    Dep,
    /// One call-site argument of a custom-hook anchor.
    Arg,
    /// One `hook_provenance` row (ADR-027 §7): a resolved hook identity.
    /// Kind-less and edge-less by design — the row survives inlining, so
    /// there may be no `hook_calls` row (and no body, no deps) behind it.
    HookOrigin,
    /// One writer of a state-hook anchor's slot (ADR-027 §1).
    Writer,
    /// One proven context-provider element (#71).
    Provider,
    /// One prop of one resolved component element (#71 step 2).
    JsxProp,
    /// One render-loop cycle of the program churn graph, carried by an effect
    /// of this component (#108).
    ChurnCycle,
    /// One prop seed of a state-hook anchor's slot (#106).
    Seed,
    /// One `useContext` call site with complete ancestry (#115).
    ContextConsumer,
}

impl Sort {
    fn describe(self) -> String {
        match self {
            Sort::Hook(None) => "a hook call (any kind)".into(),
            Sort::Hook(Some(k)) => format!("a {} hook call", kind_word(k)),
            Sort::SetterRender => "a render-body setter call".into(),
            Sort::SetterBody => "a body setter call".into(),
            Sort::Dep => "a deps entry".into(),
            Sort::Arg => "a call-site argument".into(),
            Sort::HookOrigin => "a resolved hook origin row".into(),
            Sort::Writer => "a slot writer".into(),
            Sort::Provider => "a context-provider element".into(),
            Sort::JsxProp => "a JSX prop of a component element".into(),
            Sort::ChurnCycle => "a render-loop cycle".into(),
            Sort::Seed => "a prop seed of a state slot".into(),
            Sort::ContextConsumer => "a context consumer site".into(),
        }
    }
}

fn kind_word(k: HookKindFilter) -> &'static str {
    match k {
        HookKindFilter::State => "state",
        HookKindFilter::Effect => "effect",
        HookKindFilter::Memo => "memo",
        HookKindFilter::Callback => "callback",
        HookKindFilter::Ref => "ref",
        HookKindFilter::Custom => "custom",
        HookKindFilter::Handler => "handler",
    }
}

/// Anchor kinds that carry a deps array (`effect_info` rows).
fn admits_deps(sort: Sort) -> bool {
    matches!(
        sort,
        Sort::Hook(Some(
            HookKindFilter::Effect | HookKindFilter::Memo | HookKindFilter::Callback
        ))
    )
}

// ── Resolved IR (what the executor runs) ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindRef {
    Anchor,
    /// The (single) `forEach` binding.
    Bound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MustKind {
    SetterOnAllPaths,
    DominatesAllExits,
    InitCallsSetter,
    HookIsConditional,
    DirectWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CountCmp {
    MoreThan(u64),
    LessThan(u64),
    Equals(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResolvedGuard {
    Stability {
        of: BindRef,
        names: Vec<StabilityName>,
        /// `true` when the author wrote `not` (pass ⟺ verdict ∉ names).
        negated: bool,
    },
    Returns {
        of: BindRef,
        names: Vec<ReturnsName>,
        /// `true` when the author wrote `not` (pass ⟺ verdict ∉ names).
        negated: bool,
    },
    Origin {
        of: BindRef,
        hook: Option<Vec<String>>,
        direct: Option<bool>,
    },
    InDeps {
        of: BindRef,
        negate: bool,
    },
    /// A string match on one of the subject's fields (`name`, `source`). One
    /// resolved form for every such guard: the schema keeps a distinct `kind`
    /// per field so the JSON says what it matches, the executor runs one arm.
    Text {
        of: BindRef,
        field: Field,
        one_of: Option<Vec<String>>,
        prefix: Option<String>,
    },
    /// MAY existential over the anchor slot's writers (ADR-027 §1).
    WriterPhases {
        includes: Vec<PhaseName>,
    },
    /// Identity verdict of a provider row's value (#71).
    Identity {
        of: BindRef,
        names: Vec<IdentityName>,
        negated: bool,
    },
    /// Teardown verdict of an effect anchor's own body (#100).
    Cleanup {
        of: BindRef,
        names: Vec<CleanupName>,
        negated: bool,
    },
    /// Write-provenance filter on a writer row (ADR-027 §4).
    Provenance {
        of: BindRef,
        through: Option<Vec<String>>,
        direct: Option<bool>,
    },
    /// Cardinality of `anchor.deps`.
    Count(CountCmp),
    DepsDeclared {
        eq: bool,
    },
    /// ADR-028 §2: the updater classifier of a `writers` row, a total mirror
    /// of the column so ⊤ is named rather than implied.
    Updater {
        of: BindRef,
        names: Vec<crate::rules::declarative::schema::UpdaterName>,
    },
    /// ADR-028 §2: the purity classifier derived from the same updater
    /// column, a total mirror so ⊤ is named rather than implied.
    UpdaterBody {
        of: BindRef,
        names: Vec<crate::rules::declarative::schema::ImpureName>,
    },
    /// ADR-028 §2: the row's precomputed same-tick pair fact. No value — the
    /// negative is not assertable.
    SameTick {
        of: BindRef,
    },
    /// #115: whether a provider of the consumed context is on a reaching path.
    Provider {
        of: BindRef,
        names: Vec<crate::rules::declarative::schema::ProviderName>,
    },
    /// #106: whether a seed row's slot is visibly re-synced.
    SeedSync {
        of: BindRef,
        names: Vec<crate::rules::declarative::schema::SeedSyncName>,
    },
    /// #107: who owns the slot a render-setter row writes.
    SlotOwnership {
        of: BindRef,
        names: Vec<crate::rules::declarative::schema::OwnershipName>,
    },
    /// #108: exact shape folds of a `churn_cycles` row, conjoined.
    Cycle {
        of: BindRef,
        cross_component: Option<bool>,
        all_must: Option<bool>,
    },
    Must {
        kind: MustKind,
        of: BindRef,
        els: ElseBehavior,
    },
    /// Passes when any child passes. Every child is evaluated, not
    /// short-circuited: a `must_*` child that certifies contributes its proof,
    /// and stopping at the first pass would make the finding's severity depend
    /// on the order the author happened to write the branches in.
    AnyOf(Vec<ResolvedGuard>),
    /// MAY-typed ∀ over the anchor's deps (ADR-023 §4's amendment): passes
    /// when no element definitely violates `body`. The elements are read from
    /// the anchor's own deps edge, so the variant carries only the body.
    Every(Vec<ResolvedGuard>),
}

/// A field of a bound entity — what `{binding.field}` renders and what a
/// text guard matches.
///
/// This enum is the single table both projections read: [`Field::token`] names
/// it in the schema, [`Field::admits`] says which sorts carry it, and
/// `EntityCtx::field_raw` computes it. All three are total matches on `Field`,
/// so a new field cannot be half-added — the previous split between `field_for`
/// and `render_field`, each ending in a catch-all, let a field validate and
/// then render as the empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Kind,
    Name,
    Source,
    Slot,
    Setter,
    Path,
    Stability,
    Returns,
    Region,
    Phase,
    Via,
    Identity,
    Cleanup,
    Prop,
    Cycle,
    Owner,
}

impl Field {
    /// Every variant. A variant missing here is unreachable from a template and
    /// reported as an unknown field — loud and harmless, unlike the silent
    /// empty rendering the catch-all arms used to produce.
    pub(crate) const ALL: &'static [Field] = &[
        Field::Kind,
        Field::Name,
        Field::Source,
        Field::Slot,
        Field::Setter,
        Field::Path,
        Field::Stability,
        Field::Returns,
        Field::Region,
        Field::Phase,
        Field::Via,
        Field::Identity,
        Field::Cleanup,
        Field::Prop,
        Field::Cycle,
        Field::Owner,
    ];

    /// The name authors write: `{anchor.<token>}`.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Field::Kind => "kind",
            Field::Name => "name",
            Field::Source => "source",
            Field::Slot => "slot",
            Field::Setter => "setter",
            Field::Path => "path",
            Field::Stability => "stability",
            Field::Returns => "returns",
            Field::Region => "region",
            Field::Phase => "phase",
            Field::Via => "via",
            Field::Identity => "identity",
            Field::Cleanup => "cleanup",
            Field::Prop => "prop",
            Field::Cycle => "cycle",
            Field::Owner => "owner",
        }
    }

    /// Sorts on which this field resolves. Every arm matches `sort`
    /// exhaustively, so a new sort must state its answer for every field.
    pub(crate) fn admits(self, sort: Sort) -> bool {
        match self {
            Field::Kind => match sort {
                Sort::Hook(_) => true,
                // A provenance row survives inlining precisely because it is
                // not a modeled entry — it has no HookKind.
                Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // Effects and handlers are the two kinds with nothing to call them;
            // an any-kind anchor is admitted and falls back per row.
            Field::Name => match sort {
                Sort::Hook(None) => true,
                Sort::Hook(Some(k)) => match k {
                    HookKindFilter::State
                    | HookKindFilter::Memo
                    | HookKindFilter::Callback
                    | HookKindFilter::Ref
                    | HookKindFilter::Custom => true,
                    HookKindFilter::Effect | HookKindFilter::Handler => false,
                },
                // A provenance row's only identity is its resolved origin:
                // `name` is the origin hook's name (`useLayoutEffect` even
                // through an alias), never a binding variable. A provider's
                // `name` is the context binding.
                // A consumer row is named by the local binding it reads the
                // context through — the only name the call site has.
                Sort::HookOrigin | Sort::Provider | Sort::JsxProp | Sort::ContextConsumer => true,
                // A cycle is named by its path, not by a binding.
                Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::Writer
                | Sort::ChurnCycle
                | Sort::Seed => false,
            },
            // The import specifier is recorded on custom hook rows only.
            Field::Source => match sort {
                Sort::Hook(Some(HookKindFilter::Custom)) => true,
                // The raw import specifier, recorded for every resolved call —
                // this closes the #6-noted blind spot where `source` was
                // readable on unresolved customs only.
                Sort::HookOrigin => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // A writer row is setter-shaped: it names the slot it writes and
            // the setter variable at the call site.
            Field::Slot | Field::Setter => match sort {
                Sort::SetterRender | Sort::SetterBody | Sort::Writer => true,
                Sort::Hook(_)
                | Sort::Dep
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // `stability` stays a deps-entry fact: reading it for a call-site
            // argument is the program-point error ADR-023 §2 refuses — this
            // table is where the refusal is enforced.
            // A seed row IS a prop path; a dep entry is one too. `stability`
            // is not shared: it is a render-exit verdict of a deps entry, and
            // reading it for a seed would be the program-point error ADR-023
            // §2 refuses.
            Field::Path => match sort {
                Sort::Dep | Sort::Seed => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::ContextConsumer => false,
            },
            Field::Stability => match sort {
                Sort::Dep => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            Field::Returns => match sort {
                Sort::Arg => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // `region` is the lexical body (exact); `phase` the MAY verdict;
            // `via` the wrapper chain (or `direct` / `unknown`).
            Field::Region | Field::Phase | Field::Via => match sort {
                Sort::Writer => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // Both JSX relations answer it, through the one shared
            // `site_identity` reader — and so does a call-site argument, read
            // at the call's own block (#112). NOT a deps entry: `stability`
            // is the deps fact, and reading identity there would be the same
            // program-point error §2 refuses for the reverse direction.
            Field::Identity => match sort {
                Sort::Provider | Sort::JsxProp | Sort::Arg => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            Field::Prop => match sort {
                Sort::JsxProp => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // Teardown is a property of an effect's own body: React honours a
            // returned function for effects and nothing else, so the field is
            // total only on a kind-pinned effect anchor.
            Field::Cleanup => match sort {
                Sort::Hook(Some(HookKindFilter::Effect)) => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // Which component owns the slot the row writes (#107). Total on a
            // render-setter row: local rows answer with the anchored component
            // itself. Only that sort has an owner question at all — a body
            // setter call is always the anchored component's own.
            Field::Owner => match sort {
                Sort::SetterRender => true,
                Sort::Hook(_)
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::ChurnCycle
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
            // The loop path, already node-qualified by owning component. Only
            // a cycle row has one; no other sort could invent it.
            Field::Cycle => match sort {
                Sort::ChurnCycle => true,
                Sort::Hook(_)
                | Sort::SetterRender
                | Sort::SetterBody
                | Sort::Dep
                | Sort::Arg
                | Sort::HookOrigin
                | Sort::Writer
                | Sort::Provider
                | Sort::JsxProp
                | Sort::Seed
                | Sort::ContextConsumer => false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Segment {
    Lit(String),
    Field(BindRef, Field),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolvedAnchor {
    HookCalls(Option<HookKindFilter>),
    /// `foreign` widens the enumeration with `ComponentSetter`-valued props
    /// (#107). Set by the validator iff the rule names ownership, so a pack
    /// that does not mention it binds exactly the rows it always did.
    RenderSetterCalls {
        foreign: bool,
    },
    HookOrigins,
    ContextProviders,
    /// Every prop of every resolved component element (#71 step 2).
    JsxProps,
    /// Render-loop cycles of the program churn graph carried by this
    /// component's effects (#108).
    ChurnCycles,
    /// `useContext` call sites of this component with complete ancestry (#115).
    ContextConsumers,
}

/// A fully-typed, param-baked rule — the executor never sees `PVal` or raw
/// JSON again.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedRule {
    /// Full id, `pack/rule`.
    pub id: String,
    pub pin: SeverityPin,
    pub anchor: ResolvedAnchor,
    pub for_each: Option<EdgeName>,
    pub guards: Vec<ResolvedGuard>,
    pub message: Vec<Segment>,
}

// ── Unknown-key checks over the raw JSON ──────────────────────────────────────
// serde cannot `deny_unknown_fields` on internally-tagged enums; the same
// loudness is recovered here, with exact paths, from the raw value.

fn check_keys(
    raw: &serde_json::Value,
    allowed: &[&str],
    what: &str,
    path: &str,
) -> Result<(), PackError> {
    if let serde_json::Value::Object(m) = raw {
        for key in m.keys() {
            if !allowed.contains(&key.as_str()) {
                return Err(PackError::new(
                    format!("{path}.{key}"),
                    format!(
                        "{what} does not accept field `{key}` — allowed: {}",
                        allowed.join(", ")
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn guard_allowed_keys(g: &Guard) -> (&'static str, &'static [&'static str]) {
    match g {
        Guard::Stability { .. } => ("guard `stability`", &["kind", "of", "is", "not"]),
        Guard::Returns { .. } => ("guard `returns`", &["kind", "of", "is", "not"]),
        Guard::Origin { .. } => ("guard `origin`", &["kind", "of", "hook", "direct"]),
        Guard::InDeps { .. } => ("guard `in_deps`", &["kind", "of", "negate"]),
        Guard::Name { .. } => ("guard `name`", &["kind", "of", "one_of", "prefix"]),
        Guard::Source { .. } => ("guard `source`", &["kind", "of", "one_of", "prefix"]),
        Guard::WriterPhases { .. } => ("guard `writer_phases`", &["kind", "of", "includes"]),
        Guard::Provenance { .. } => ("guard `provenance`", &["kind", "of", "through", "direct"]),
        Guard::Identity { .. } => ("guard `identity`", &["kind", "of", "is", "not"]),
        Guard::Cleanup { .. } => ("guard `cleanup`", &["kind", "of", "is", "not"]),
        Guard::Count { .. } => (
            "guard `count`",
            &["kind", "of", "more_than", "less_than", "equals"],
        ),
        Guard::DepsDeclared { .. } => ("guard `deps_declared`", &["kind", "of", "eq"]),
        Guard::MustSetterOnAllPaths { .. } => {
            ("guard `must_setter_on_all_paths`", &["kind", "of", "else"])
        }
        Guard::MustDominatesAllExits { .. } => {
            ("guard `must_dominates_all_exits`", &["kind", "of", "else"])
        }
        Guard::MustInitCallsSetter { .. } => {
            ("guard `must_init_calls_setter`", &["kind", "of", "else"])
        }
        Guard::MustHookIsConditional { .. } => {
            ("guard `must_hook_is_conditional`", &["kind", "of", "else"])
        }
        Guard::MustDirectWrite { .. } => ("guard `must_direct_write`", &["kind", "of", "else"]),
        Guard::Updater { .. } => ("guard `updater`", &["kind", "of", "is"]),
        Guard::UpdaterBody { .. } => ("guard `updater_body`", &["kind", "of", "is"]),
        Guard::SameTick { .. } => ("guard `same_tick`", &["kind", "of"]),
        Guard::Provider { .. } => ("guard `provider`", &["kind", "of", "is"]),
        Guard::SeedSync { .. } => ("guard `seed_sync`", &["kind", "of", "is"]),
        Guard::SlotOwnership { .. } => ("guard `slot_ownership`", &["kind", "of", "is"]),
        Guard::Cycle { .. } => (
            "guard `cycle`",
            &["kind", "of", "cross_component", "all_must"],
        ),
        Guard::AnyOf { .. } => ("guard `any_of`", &["kind", "guards"]),
        Guard::Every { .. } => ("guard `every`", &["kind", "of", "as", "guards"]),
    }
}

// ── Param machinery (ADR-022 §4) ──────────────────────────────────────────────

fn value_matches(v: &serde_json::Value, ty: ParamType) -> bool {
    match ty {
        ParamType::Number => v.is_number(),
        ParamType::String => v.is_string(),
        ParamType::Boolean => v.is_boolean(),
        ParamType::StringList => v
            .as_array()
            .is_some_and(|a| a.iter().all(|x| x.is_string())),
    }
}

fn type_word(ty: ParamType) -> &'static str {
    match ty {
        ParamType::Number => "number",
        ParamType::String => "string",
        ParamType::Boolean => "boolean",
        ParamType::StringList => "string[]",
    }
}

/// Effective param values (declared defaults overridden by config options),
/// plus a used-set so unused declarations warn.
struct ParamEnv<'a> {
    decls: &'a BTreeMap<String, ParamDecl>,
    values: BTreeMap<String, serde_json::Value>,
    used: std::cell::RefCell<std::collections::BTreeSet<String>>,
}

impl<'a> ParamEnv<'a> {
    fn build(
        decls: &'a BTreeMap<String, ParamDecl>,
        options: Option<&serde_json::Map<String, serde_json::Value>>,
        path: &str,
    ) -> Result<Self, PackError> {
        for (name, decl) in decls {
            if !value_matches(&decl.default, decl.ty) {
                return Err(PackError::new(
                    format!("{path}.params.{name}.default"),
                    format!(
                        "default {} does not match declared type `{}`",
                        decl.default,
                        type_word(decl.ty)
                    ),
                ));
            }
        }
        let mut values: BTreeMap<String, serde_json::Value> = decls
            .iter()
            .map(|(k, d)| (k.clone(), d.default.clone()))
            .collect();
        if let Some(options) = options {
            for (key, value) in options {
                let Some(decl) = decls.get(key) else {
                    return Err(PackError::new(
                        format!("{path}.options.{key}"),
                        format!(
                            "unknown option `{key}` — declared params: {}",
                            if decls.is_empty() {
                                "none".to_string()
                            } else {
                                decls.keys().cloned().collect::<Vec<_>>().join(", ")
                            }
                        ),
                    ));
                };
                if !value_matches(value, decl.ty) {
                    return Err(PackError::new(
                        format!("{path}.options.{key}"),
                        format!(
                            "option value {value} does not match declared type `{}`",
                            type_word(decl.ty)
                        ),
                    ));
                }
                values.insert(key.clone(), value.clone());
            }
        }
        Ok(ParamEnv {
            decls,
            values,
            used: Default::default(),
        })
    }

    /// Resolve a `PVal` in a position that admits params of type `expected`
    /// (`None` = the position takes no param at all).
    fn resolve<T: serde::de::DeserializeOwned + Clone>(
        &self,
        pv: &PVal<T>,
        expected: Option<ParamType>,
        path: &str,
    ) -> Result<T, PackError> {
        match pv {
            PVal::Value(v) => Ok(v.clone()),
            PVal::Param(name) => {
                let Some(expected) = expected else {
                    return Err(PackError::new(
                        path,
                        "this position does not accept a {\"$param\": …} reference",
                    ));
                };
                let Some(decl) = self.decls.get(name) else {
                    return Err(PackError::new(
                        path,
                        format!("reference to undeclared param `{name}`"),
                    ));
                };
                if decl.ty != expected {
                    return Err(PackError::new(
                        path,
                        format!(
                            "param `{name}` has type `{}`, but this position expects `{}`",
                            type_word(decl.ty),
                            type_word(expected)
                        ),
                    ));
                }
                self.used.borrow_mut().insert(name.clone());
                serde_json::from_value(self.values[name].clone()).map_err(|e| {
                    PackError::new(path, format!("param `{name}` value is unusable here: {e}"))
                })
            }
        }
    }

    /// Render a param for template interpolation.
    fn render(&self, name: &str, path: &str) -> Result<String, PackError> {
        let Some(value) = self.values.get(name) else {
            return Err(PackError::new(
                path,
                format!("reference to undeclared param `{name}` in message template"),
            ));
        };
        self.used.borrow_mut().insert(name.to_string());
        Ok(match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            other => other.to_string(),
        })
    }
}

// ── The validator ─────────────────────────────────────────────────────────────

/// Validate one rule against the sort system and bake its params. `raw_rule`
/// is the rule's raw JSON (for unknown-key checks inside guards/anchor).
fn validate_rule(
    pack_name: &str,
    index: usize,
    def: &RuleDef,
    raw_rule: &serde_json::Value,
    options: Option<&serde_json::Map<String, serde_json::Value>>,
    warnings: &mut Vec<LoadWarning>,
) -> Result<ResolvedRule, PackError> {
    let path = format!("rules[{index}]");
    let full_id = format!("{pack_name}/{}", def.id);

    // Identity + docs (ADR-022 §5).
    if def.id.is_empty() {
        return Err(PackError::new(format!("{path}.id"), "rule id is empty"));
    }
    if def.id.contains('/') {
        return Err(PackError::new(
            format!("{path}.id"),
            format!(
                "rule id `{}` contains `/` — the pack name is the namespace, ids are bare",
                def.id
            ),
        ));
    }
    for (field, value) in [
        ("description", &def.docs.description),
        ("why", &def.docs.why),
        ("fix", &def.docs.fix),
    ] {
        if value.trim().is_empty() {
            return Err(PackError::new(
                format!("{path}.docs.{field}"),
                format!("docs are mandatory: `{field}` is empty"),
            ));
        }
    }

    // Anchor.
    check_keys(
        &raw_rule["anchor"],
        match def.anchor {
            Anchor::HookCalls { .. } => &["relation", "kind"],
            Anchor::RenderSetterCalls
            | Anchor::HookOrigins
            | Anchor::ContextProviders
            | Anchor::JsxProps
            | Anchor::ChurnCycles
            | Anchor::ContextConsumers => &["relation"],
        },
        "this anchor",
        &format!("{path}.anchor"),
    )?;
    let (anchor, anchor_sort) = match &def.anchor {
        Anchor::HookCalls { kind } => (ResolvedAnchor::HookCalls(*kind), Sort::Hook(*kind)),
        Anchor::RenderSetterCalls => (
            // Patched below once the guards are known: only a rule that names
            // ownership gets the widened enumeration.
            ResolvedAnchor::RenderSetterCalls { foreign: false },
            Sort::SetterRender,
        ),
        Anchor::HookOrigins => (ResolvedAnchor::HookOrigins, Sort::HookOrigin),
        Anchor::ContextProviders => (ResolvedAnchor::ContextProviders, Sort::Provider),
        Anchor::JsxProps => (ResolvedAnchor::JsxProps, Sort::JsxProp),
        Anchor::ChurnCycles => (ResolvedAnchor::ChurnCycles, Sort::ChurnCycle),
        Anchor::ContextConsumers => (ResolvedAnchor::ContextConsumers, Sort::ContextConsumer),
    };

    // forEach: at most one typed edge, one binding (ADR-022 §2).
    let mut bound_sort: Option<Sort> = None;
    let mut bound_name: Option<&str> = None;
    if let Some(fe) = &def.for_each {
        let fe_path = format!("{path}.forEach");
        if fe.bind == "anchor" || fe.bind == "param" || fe.bind.is_empty() || fe.bind.contains('.')
        {
            return Err(PackError::new(
                format!("{fe_path}.as"),
                format!("`{}` is not a usable binding name", fe.bind),
            ));
        }
        let element = match fe.edge {
            EdgeName::Deps => {
                if !admits_deps(anchor_sort) {
                    return Err(PackError::new(
                        format!("{fe_path}.edge"),
                        format!(
                            "edge `deps` needs an effect/memo/callback anchor, but the anchor \
                             binds {}",
                            anchor_sort.describe()
                        ),
                    ));
                }
                Sort::Dep
            }
            EdgeName::BodySetterCalls => {
                if !matches!(
                    anchor_sort,
                    Sort::Hook(Some(
                        HookKindFilter::Effect
                            | HookKindFilter::Memo
                            | HookKindFilter::Callback
                            | HookKindFilter::Handler
                    ))
                ) {
                    return Err(PackError::new(
                        format!("{fe_path}.edge"),
                        format!(
                            "edge `body_setter_calls` needs an anchor with a body \
                             (effect/memo/callback/handler), but the anchor binds {}",
                            anchor_sort.describe()
                        ),
                    ));
                }
                Sort::SetterBody
            }
            EdgeName::Args => {
                if !matches!(anchor_sort, Sort::Hook(Some(HookKindFilter::Custom))) {
                    return Err(PackError::new(
                        format!("{fe_path}.edge"),
                        format!(
                            "edge `args` needs a custom-hook anchor, but the anchor binds {}",
                            anchor_sort.describe()
                        ),
                    ));
                }
                Sort::Arg
            }
            EdgeName::Writers => {
                if !matches!(anchor_sort, Sort::Hook(Some(HookKindFilter::State))) {
                    return Err(PackError::new(
                        format!("{fe_path}.edge"),
                        format!(
                            "edge `writers` needs a state-hook anchor (the slot whose \
                             writers it enumerates), but the anchor binds {}",
                            anchor_sort.describe()
                        ),
                    ));
                }
                Sort::Writer
            }
            EdgeName::Seeds => {
                if !matches!(anchor_sort, Sort::Hook(Some(HookKindFilter::State))) {
                    return Err(PackError::new(
                        format!("{fe_path}.edge"),
                        format!(
                            "edge `seeds` needs a state-hook anchor (the slot whose \
                             initializer it reads), but the anchor binds {}",
                            anchor_sort.describe()
                        ),
                    ));
                }
                Sort::Seed
            }
        };
        bound_sort = Some(element);
        bound_name = Some(fe.bind.as_str());
    }

    let env = ParamEnv::build(&def.params, options, &path)?;
    let cx = GuardCx {
        anchor_sort,
        bound_name,
        bound_sort,
        env: &env,
    };

    // Guards.
    let mut guards = Vec::with_capacity(def.guards.len());
    let mut has_must = false;
    let mut guard_warnings: Vec<String> = Vec::new();
    for (gi, guard) in def.guards.iter().enumerate() {
        let g_path = format!("{path}.guards[{gi}]");
        guards.push(validate_guard(
            guard,
            &raw_rule["guards"][gi],
            &g_path,
            &cx,
            &mut has_must,
            &mut guard_warnings,
        )?);
    }

    // A may-typed quantifier can select a row on the strength of ⊤ elements
    // alone, so a rule that uses one must not also carry Error authority: with
    // no `must_*` anywhere, the finding stratifies to Warning structurally
    // rather than by policy (ADR-023 §4's amendment).
    if has_must && guards.iter().any(quantifies) {
        return Err(PackError::new(
            format!("{path}.guards"),
            "a rule using `every` cannot also use a `must_*` guard: the quantifier is \
             may-typed, so a row it selected cannot carry a certified claim",
        ));
    }

    // #107: the render-setter enumeration widens ONLY for a rule that names
    // ownership. Anywhere in the tree, `any_of` included — a foreign row a
    // disjunct can select must exist for that disjunct to see it.
    let anchor = match anchor {
        ResolvedAnchor::RenderSetterCalls { .. } => ResolvedAnchor::RenderSetterCalls {
            foreign: guards.iter().any(names_ownership),
        },
        other => other,
    };

    // Message template.
    let message = parse_template(
        &def.message,
        anchor_sort,
        bound_name,
        bound_sort,
        &env,
        &format!("{path}.message"),
    )?;

    for message in guard_warnings {
        warnings.push(LoadWarning {
            rule: full_id.clone(),
            message,
        });
    }
    // §3: the only static severity check is a WARNING, never a rejection —
    // a pin of "error" with no must-guard can never be reached.
    if def.severity == SeverityPin::Error && !has_must {
        warnings.push(LoadWarning {
            rule: full_id.clone(),
            message: "severity is pinned \"error\" but no must_* guard is used — findings can \
                      only emit as warnings"
                .into(),
        });
    }
    // #6 (ADR-027 §7): a `kind: "custom"` anchor binds only the hooks the
    // engine could NOT resolve — expansion removes the row — so an identity
    // rule written on it silently under-reports exactly when the analysis
    // gets better. Warn only when the rule can actually move: no edge (origin
    // rows are edge-less) and every guard expressible there — a rule that
    // needs `args` has no better formulation, so the blindness is a recorded
    // limitation for it, not an actionable warning.
    if matches!(
        anchor,
        ResolvedAnchor::HookCalls(Some(HookKindFilter::Custom))
    ) && def.for_each.is_none()
        && !guards.is_empty()
        && guards.iter().all(anchor_identity_guard)
    {
        warnings.push(LoadWarning {
            rule: full_id.clone(),
            message: "a `kind: \"custom\"` anchor only binds hooks the engine could not \
                      resolve (#6) — identity rules belong on the `hook_origins` anchor, \
                      which survives inlining"
                .into(),
        });
    }
    // Unused params load fine, but the author probably meant something.
    let used = env.used.into_inner();
    for name in def.params.keys() {
        if !used.contains(name) {
            warnings.push(LoadWarning {
                rule: full_id.clone(),
                message: format!("param `{name}` is declared but never referenced"),
            });
        }
    }

    Ok(ResolvedRule {
        id: full_id,
        pin: def.severity,
        anchor,
        for_each: def.for_each.as_ref().map(|f| f.edge),
        guards,
        message,
    })
}

/// Does this guard tree name slot ownership anywhere (#107)? The trigger for
/// the widened render-setter enumeration.
fn names_ownership(g: &ResolvedGuard) -> bool {
    match g {
        ResolvedGuard::SlotOwnership { .. } => true,
        ResolvedGuard::AnyOf(children) | ResolvedGuard::Every(children) => {
            children.iter().any(names_ownership)
        }
        _ => false,
    }
}

/// Does this guard tree contain a `every` quantifier anywhere?
fn quantifies(g: &ResolvedGuard) -> bool {
    match g {
        ResolvedGuard::Every(_) => true,
        ResolvedGuard::AnyOf(children) => children.iter().any(quantifies),
        _ => false,
    }
}

/// Does this guard (or any `any_of` branch of it) match the ANCHOR's
/// identity (`name` / `source` / `origin`)? The trigger of the #6 warning.
fn anchor_identity_guard(g: &ResolvedGuard) -> bool {
    match g {
        ResolvedGuard::Text { of, field, .. } => {
            *of == BindRef::Anchor && matches!(field, Field::Name | Field::Source)
        }
        ResolvedGuard::Origin { of, .. } => *of == BindRef::Anchor,
        ResolvedGuard::AnyOf(children) => children.iter().any(anchor_identity_guard),
        _ => false,
    }
}

/// Everything a guard is validated against: the anchor's sort, the `forEach`
/// binding if there is one, and the resolved params.
struct GuardCx<'a> {
    anchor_sort: Sort,
    bound_name: Option<&'a str>,
    bound_sort: Option<Sort>,
    env: &'a ParamEnv<'a>,
}

impl GuardCx<'_> {
    /// A guard's subject: "anchor" or the forEach binding.
    fn resolve_of(&self, of: &str, g_path: &str) -> Result<(BindRef, Sort), PackError> {
        if of == "anchor" {
            Ok((BindRef::Anchor, self.anchor_sort))
        } else if Some(of) == self.bound_name {
            Ok((BindRef::Bound, self.bound_sort.unwrap()))
        } else {
            Err(PackError::new(
                format!("{g_path}.of"),
                match self.bound_name {
                    Some(b) => format!("unknown binding `{of}` — available: anchor, {b}"),
                    None => format!("unknown binding `{of}` — available: anchor"),
                },
            ))
        }
    }
}

/// Validate one guard, recursively for `any_of`. `has_must` accumulates over
/// the whole tree: a certifying guard anywhere in it can mint an Error, so the
/// §3 "pinned error with no must_*" warning must see through disjunctions.
fn validate_guard(
    guard: &Guard,
    raw: &serde_json::Value,
    g_path: &str,
    cx: &GuardCx<'_>,
    has_must: &mut bool,
    warnings: &mut Vec<String>,
) -> Result<ResolvedGuard, PackError> {
    let (what, allowed) = guard_allowed_keys(guard);
    check_keys(raw, allowed, what, g_path)?;
    Ok(match guard {
        Guard::Stability { of, is, not } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Dep {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `stability` applies to a deps entry, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let (names, negated) = match (is, not) {
                (Some(pv), None) => (cx.env.resolve(pv, None, &format!("{g_path}.is"))?, false),
                (None, Some(pv)) => (cx.env.resolve(pv, None, &format!("{g_path}.not"))?, true),
                _ => {
                    return Err(PackError::new(
                        g_path,
                        "guard `stability` takes exactly one of `is` / `not`",
                    ));
                }
            };
            if names.is_empty() {
                return Err(PackError::new(g_path, "the verdict list must not be empty"));
            }
            ResolvedGuard::Stability { of, names, negated }
        }
        Guard::Returns { of, is, not } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Arg {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `returns` applies to a call-site argument (the `args` edge), \
                         but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let (names, negated) = match (is, not) {
                (Some(pv), None) => (cx.env.resolve(pv, None, &format!("{g_path}.is"))?, false),
                (None, Some(pv)) => (cx.env.resolve(pv, None, &format!("{g_path}.not"))?, true),
                _ => {
                    return Err(PackError::new(
                        g_path,
                        "guard `returns` takes exactly one of `is` / `not`",
                    ));
                }
            };
            if names.is_empty() {
                return Err(PackError::new(g_path, "the verdict list must not be empty"));
            }
            ResolvedGuard::Returns { of, names, negated }
        }
        Guard::Origin { of, hook, direct } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if !matches!(sort, Sort::Hook(_) | Sort::HookOrigin) {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `origin` applies to a hook-call row, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let hook = match hook {
                Some(pv) => Some(cx.env.resolve(
                    pv,
                    Some(ParamType::StringList),
                    &format!("{g_path}.hook"),
                )?),
                None => None,
            };
            let direct = match direct {
                Some(pv) => Some(cx.env.resolve(
                    pv,
                    Some(ParamType::Boolean),
                    &format!("{g_path}.direct"),
                )?),
                None => None,
            };
            if hook.is_none() && direct.is_none() {
                return Err(PackError::new(
                    g_path,
                    "guard `origin` needs at least one of `hook` / `direct`",
                ));
            }
            if hook.as_ref().is_some_and(|h| h.is_empty()) {
                return Err(PackError::new(g_path, "the hook list must not be empty"));
            }
            ResolvedGuard::Origin { of, hook, direct }
        }
        Guard::InDeps { of, negate } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::SetterBody {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `in_deps` applies to a body setter call, but the subject \
                         binds {}",
                        sort.describe()
                    ),
                ));
            }
            if !admits_deps(cx.anchor_sort) {
                return Err(PackError::new(
                    g_path,
                    format!(
                        "guard `in_deps` needs an anchor with a deps array \
                         (effect/memo/callback), but the anchor binds {}",
                        cx.anchor_sort.describe()
                    ),
                ));
            }
            ResolvedGuard::InDeps {
                of,
                negate: *negate,
            }
        }
        // `name` and `source` are one matcher over two fields. The subject
        // check is `Field::admits` — the same table the templates use, so
        // a field can never be renderable but unguardable (or the reverse).
        Guard::Name { of, one_of, prefix } => {
            text_guard(Field::Name, of, one_of, prefix, cx, g_path)?
        }
        Guard::Source { of, one_of, prefix } => {
            text_guard(Field::Source, of, one_of, prefix, cx, g_path)?
        }
        Guard::WriterPhases { of, includes } => {
            let (of_ref, sort) = cx.resolve_of(of, g_path)?;
            if of_ref != BindRef::Anchor || !matches!(sort, Sort::Hook(Some(HookKindFilter::State)))
            {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `writer_phases` reads the writers of a state-hook ANCHOR's \
                         slot, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let includes = cx
                .env
                .resolve(includes, None, &format!("{g_path}.includes"))?;
            if includes.is_empty() {
                return Err(PackError::new(
                    format!("{g_path}.includes"),
                    "the phase list must not be empty",
                ));
            }
            ResolvedGuard::WriterPhases { includes }
        }
        Guard::Identity { of, is, not } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if !matches!(sort, Sort::Provider | Sort::JsxProp | Sort::Arg) {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `identity` applies to a context-provider element, a JSX prop \
                         or a call-site argument, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let (names, negated) = match (is, not) {
                (Some(pv), None) => (cx.env.resolve(pv, None, &format!("{g_path}.is"))?, false),
                (None, Some(pv)) => (cx.env.resolve(pv, None, &format!("{g_path}.not"))?, true),
                _ => {
                    return Err(PackError::new(
                        g_path,
                        "guard `identity` takes exactly one of `is` / `not`",
                    ));
                }
            };
            if names.is_empty() {
                return Err(PackError::new(g_path, "the verdict list must not be empty"));
            }
            ResolvedGuard::Identity { of, names, negated }
        }
        Guard::Cleanup { of, is, not } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Hook(Some(HookKindFilter::Effect)) {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `cleanup` applies to a `kind: \"effect\"` hook anchor, but the \
                         subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let (names, negated) = match (is, not) {
                (Some(pv), None) => (cx.env.resolve(pv, None, &format!("{g_path}.is"))?, false),
                (None, Some(pv)) => (cx.env.resolve(pv, None, &format!("{g_path}.not"))?, true),
                _ => {
                    return Err(PackError::new(
                        g_path,
                        "guard `cleanup` takes exactly one of `is` / `not`",
                    ));
                }
            };
            if names.is_empty() {
                return Err(PackError::new(g_path, "the verdict list must not be empty"));
            }
            ResolvedGuard::Cleanup { of, names, negated }
        }
        Guard::Updater { of, is } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Writer {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `updater` applies to a `writers` row, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let names = cx.env.resolve(is, None, &format!("{g_path}.is"))?;
            if names.is_empty() {
                return Err(PackError::new(
                    format!("{g_path}.is"),
                    "guard `updater` needs at least one verdict name",
                ));
            }
            ResolvedGuard::Updater { of, names }
        }
        Guard::UpdaterBody { of, is } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Writer {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `updater_body` applies to a `writers` row, but the subject \
                         binds {}",
                        sort.describe()
                    ),
                ));
            }
            let names = cx.env.resolve(is, None, &format!("{g_path}.is"))?;
            if names.is_empty() {
                return Err(PackError::new(
                    format!("{g_path}.is"),
                    "guard `updater_body` needs at least one verdict name",
                ));
            }
            ResolvedGuard::UpdaterBody { of, names }
        }
        Guard::SameTick { of } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Writer {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `same_tick` applies to a `writers` row, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            ResolvedGuard::SameTick { of }
        }
        Guard::Provider { of, is } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::ContextConsumer {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `provider` applies to a `context_consumers` row, but the \
                         subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let names = cx.env.resolve(is, None, &format!("{g_path}.is"))?;
            if names.is_empty() {
                return Err(PackError::new(
                    format!("{g_path}.is"),
                    "guard `provider` needs at least one verdict name",
                ));
            }
            ResolvedGuard::Provider { of, names }
        }
        Guard::SeedSync { of, is } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Seed {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `seed_sync` applies to a `seeds` row, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let names = cx.env.resolve(is, None, &format!("{g_path}.is"))?;
            if names.is_empty() {
                return Err(PackError::new(
                    format!("{g_path}.is"),
                    "guard `seed_sync` needs at least one verdict name",
                ));
            }
            ResolvedGuard::SeedSync { of, names }
        }
        Guard::SlotOwnership { of, is } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::SetterRender {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `slot_ownership` applies to a `render_setter_calls` row, but \
                         the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let names = cx.env.resolve(is, None, &format!("{g_path}.is"))?;
            if names.is_empty() {
                return Err(PackError::new(
                    format!("{g_path}.is"),
                    "guard `slot_ownership` needs at least one ownership name",
                ));
            }
            ResolvedGuard::SlotOwnership { of, names }
        }
        Guard::Cycle {
            of,
            cross_component,
            all_must,
        } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::ChurnCycle {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `cycle` applies to a `churn_cycles` row, but the subject \
                         binds {}",
                        sort.describe()
                    ),
                ));
            }
            let cross_component = match cross_component {
                Some(pv) => Some(cx.env.resolve(
                    pv,
                    Some(ParamType::Boolean),
                    &format!("{g_path}.cross_component"),
                )?),
                None => None,
            };
            let all_must = match all_must {
                Some(pv) => Some(cx.env.resolve(
                    pv,
                    Some(ParamType::Boolean),
                    &format!("{g_path}.all_must"),
                )?),
                None => None,
            };
            if cross_component.is_none() && all_must.is_none() {
                return Err(PackError::new(
                    g_path,
                    "guard `cycle` needs at least one of `cross_component` / `all_must`",
                ));
            }
            ResolvedGuard::Cycle {
                of,
                cross_component,
                all_must,
            }
        }
        Guard::Provenance {
            of,
            through,
            direct,
        } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Writer {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `provenance` applies to a `writers` row, but the subject \
                         binds {}",
                        sort.describe()
                    ),
                ));
            }
            let through = match through {
                Some(pv) => Some(cx.env.resolve(
                    pv,
                    Some(ParamType::StringList),
                    &format!("{g_path}.through"),
                )?),
                None => None,
            };
            let direct = match direct {
                Some(pv) => Some(cx.env.resolve(
                    pv,
                    Some(ParamType::Boolean),
                    &format!("{g_path}.direct"),
                )?),
                None => None,
            };
            if through.is_none() && direct.is_none() {
                return Err(PackError::new(
                    g_path,
                    "guard `provenance` needs at least one of `through` / `direct`",
                ));
            }
            if through.as_ref().is_some_and(|t| t.is_empty()) {
                return Err(PackError::new(g_path, "the wrapper list must not be empty"));
            }
            ResolvedGuard::Provenance {
                of,
                through,
                direct,
            }
        }
        Guard::Count {
            of,
            more_than,
            less_than,
            equals,
        } => {
            if of != "anchor.deps" {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!("guard `count` counts `anchor.deps` (got `{of}`)"),
                ));
            }
            if !admits_deps(cx.anchor_sort) {
                return Err(PackError::new(
                    g_path,
                    format!(
                        "guard `count` needs an anchor with a deps array \
                         (effect/memo/callback), but the anchor binds {}",
                        cx.anchor_sort.describe()
                    ),
                ));
            }
            let cmp = match (more_than, less_than, equals) {
                (Some(pv), None, None) => CountCmp::MoreThan(cx.env.resolve(
                    pv,
                    Some(ParamType::Number),
                    &format!("{g_path}.more_than"),
                )?),
                (None, Some(pv), None) => CountCmp::LessThan(cx.env.resolve(
                    pv,
                    Some(ParamType::Number),
                    &format!("{g_path}.less_than"),
                )?),
                (None, None, Some(pv)) => CountCmp::Equals(cx.env.resolve(
                    pv,
                    Some(ParamType::Number),
                    &format!("{g_path}.equals"),
                )?),
                _ => {
                    return Err(PackError::new(
                        g_path,
                        "guard `count` takes exactly one of `more_than` / `less_than` / \
                         `equals`",
                    ));
                }
            };
            ResolvedGuard::Count(cmp)
        }
        Guard::DepsDeclared { of, eq } => {
            let (of_ref, sort) = cx.resolve_of(of, g_path)?;
            if of_ref != BindRef::Anchor || !admits_deps(sort) {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    "guard `deps_declared` applies to an effect/memo/callback anchor",
                ));
            }
            ResolvedGuard::DepsDeclared {
                eq: cx
                    .env
                    .resolve(eq, Some(ParamType::Boolean), &format!("{g_path}.eq"))?,
            }
        }
        Guard::MustSetterOnAllPaths { of, r#else } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::SetterBody {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `must_setter_on_all_paths` applies to a body setter call, \
                         but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            *has_must = true;
            ResolvedGuard::Must {
                kind: MustKind::SetterOnAllPaths,
                of,
                els: *r#else,
            }
        }
        Guard::MustDominatesAllExits { of, r#else } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::SetterRender {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `must_dominates_all_exits` applies to a render setter call, \
                         but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            *has_must = true;
            ResolvedGuard::Must {
                kind: MustKind::DominatesAllExits,
                of,
                els: *r#else,
            }
        }
        Guard::MustInitCallsSetter { of, r#else } => {
            let (of_ref, sort) = cx.resolve_of(of, g_path)?;
            // The two hooks that take an initializer React evaluates on
            // every render; native `lazy-init` covers exactly the same pair.
            let has_init = matches!(
                sort,
                Sort::Hook(Some(HookKindFilter::State | HookKindFilter::Ref))
            );
            if of_ref != BindRef::Anchor || !has_init {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    "guard `must_init_calls_setter` applies to a state- or ref-hook anchor",
                ));
            }
            *has_must = true;
            ResolvedGuard::Must {
                kind: MustKind::InitCallsSetter,
                of: of_ref,
                els: *r#else,
            }
        }
        Guard::MustHookIsConditional { of, r#else } => {
            let (of_ref, sort) = cx.resolve_of(of, g_path)?;
            if of_ref != BindRef::Anchor || !matches!(sort, Sort::Hook(_)) {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    "guard `must_hook_is_conditional` applies to the hook-call anchor",
                ));
            }
            *has_must = true;
            ResolvedGuard::Must {
                kind: MustKind::HookIsConditional,
                of: of_ref,
                els: *r#else,
            }
        }
        Guard::MustDirectWrite { of, r#else } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Writer {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `must_direct_write` applies to a `writers` row, but the \
                         subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            *has_must = true;
            ResolvedGuard::Must {
                kind: MustKind::DirectWrite,
                of,
                els: *r#else,
            }
        }
        Guard::AnyOf { guards } => {
            if guards.len() < 2 {
                return Err(PackError::new(
                    format!("{g_path}.guards"),
                    format!(
                        "guard `any_of` needs at least two alternatives (got {})",
                        guards.len()
                    ),
                ));
            }
            let mut children = Vec::with_capacity(guards.len());
            for (i, child) in guards.iter().enumerate() {
                let c_path = format!("{g_path}.guards[{i}]");
                // A `must_*` branch left on the default `else: keep` passes
                // whether or not it certifies, so the disjunction is always
                // true and every other branch is dead. Loud, but a warning:
                // over-reporting is the tolerated direction.
                if let Guard::MustSetterOnAllPaths { r#else, .. }
                | Guard::MustDominatesAllExits { r#else, .. }
                | Guard::MustInitCallsSetter { r#else, .. }
                | Guard::MustHookIsConditional { r#else, .. } = child
                    && *r#else == ElseBehavior::Keep
                {
                    warnings.push(format!(
                        "at `{c_path}`: a `must_*` branch of `any_of` with the default \
                         `\"else\": \"keep\"` always passes, so the disjunction is always true \
                         — add `\"else\": \"drop\"` if the branch is meant to be a condition"
                    ));
                }
                children.push(validate_guard(
                    child,
                    &raw["guards"][i],
                    &c_path,
                    cx,
                    has_must,
                    warnings,
                )?);
            }
            ResolvedGuard::AnyOf(children)
        }
        Guard::Every { of, r#as, guards } => {
            // Same subject spelling as `count`: an edge, not a binding.
            if of != "anchor.deps" {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!("guard `every` quantifies over `anchor.deps` (got `{of}`)"),
                ));
            }
            if !admits_deps(cx.anchor_sort) {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `every` quantifies over the deps of an effect/memo/callback \
                         anchor, but the anchor binds {}",
                        cx.anchor_sort.describe()
                    ),
                ));
            }
            if guards.is_empty() {
                return Err(PackError::new(
                    format!("{g_path}.guards"),
                    "guard `every` needs at least one guard to quantify",
                ));
            }
            // The quantifier owns the element slot for its own subtree: inside
            // it, `as` is the binding and the rule's `forEach` name is not
            // reachable. One slot, so one visible element at a time.
            let inner = GuardCx {
                anchor_sort: cx.anchor_sort,
                bound_name: Some(r#as),
                bound_sort: Some(Sort::Dep),
                env: cx.env,
            };
            let mut body = Vec::with_capacity(guards.len());
            for (i, child) in guards.iter().enumerate() {
                // A `must_*` inside the quantifier would mint a proof for a
                // row a may-fact selected. Refused at load time rather than
                // dropped at exec: the author gets told, not silently ignored.
                let mut child_must = false;
                let resolved = validate_guard(
                    child,
                    &raw["guards"][i],
                    &format!("{g_path}.guards[{i}]"),
                    &inner,
                    &mut child_must,
                    warnings,
                )?;
                if child_must {
                    return Err(PackError::new(
                        format!("{g_path}.guards[{i}]"),
                        "a `must_*` guard cannot appear inside `every`: the quantifier is \
                         may-typed, so a row it selected cannot carry a certified claim",
                    ));
                }
                body.push(resolved);
            }
            ResolvedGuard::Every(body)
        }
    })
}

/// Fields available per sort — the template's type check, derived from the one
/// `Field` table so validation and rendering cannot disagree.
fn field_for(sort: Sort, name: &str) -> Option<Field> {
    Field::ALL
        .iter()
        .copied()
        .find(|f| f.token() == name && f.admits(sort))
}

/// Validate a string-matching guard (`name`, `source`) against `field`. The
/// subject must be a sort that carries the field, and exactly one of
/// `one_of`/`prefix` must be given.
fn text_guard(
    field: Field,
    of: &str,
    one_of: &Option<PVal<Vec<String>>>,
    prefix: &Option<PVal<String>>,
    cx: &GuardCx<'_>,
    g_path: &str,
) -> Result<ResolvedGuard, PackError> {
    let kind = field.token();
    let (of, sort) = cx.resolve_of(of, g_path)?;
    if !field.admits(sort) {
        return Err(PackError::new(
            format!("{g_path}.of"),
            format!(
                "guard `{kind}` matches the `{kind}` field, which {} does not carry — its \
                 fields: {}",
                sort.describe(),
                match fields_of(sort).as_slice() {
                    [] => "none".to_string(),
                    fs => fs.join(", "),
                }
            ),
        ));
    }
    match (one_of, prefix) {
        (Some(pv), None) => Ok(ResolvedGuard::Text {
            of,
            field,
            one_of: Some(cx.env.resolve(
                pv,
                Some(ParamType::StringList),
                &format!("{g_path}.one_of"),
            )?),
            prefix: None,
        }),
        (None, Some(pv)) => Ok(ResolvedGuard::Text {
            of,
            field,
            one_of: None,
            prefix: Some(cx.env.resolve(
                pv,
                Some(ParamType::String),
                &format!("{g_path}.prefix"),
            )?),
        }),
        _ => Err(PackError::new(
            g_path,
            format!("guard `{kind}` takes exactly one of `one_of` / `prefix`"),
        )),
    }
}

/// The fields this sort carries, for the "unknown field" message.
fn fields_of(sort: Sort) -> Vec<&'static str> {
    Field::ALL
        .iter()
        .filter(|f| f.admits(sort))
        .map(|f| f.token())
        .collect()
}

fn parse_template(
    template: &str,
    anchor_sort: Sort,
    bound_name: Option<&str>,
    bound_sort: Option<Sort>,
    env: &ParamEnv,
    path: &str,
) -> Result<Vec<Segment>, PackError> {
    let mut segments = Vec::new();
    let mut lit = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                lit.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                lit.push('}');
            }
            '{' => {
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => inner.push(c),
                        None => {
                            return Err(PackError::new(
                                path,
                                format!("unclosed `{{` in message template (at `{{{inner}`)"),
                            ));
                        }
                    }
                }
                let Some((binding, field)) = inner.split_once('.') else {
                    return Err(PackError::new(
                        path,
                        format!(
                            "template placeholder `{{{inner}}}` must be `binding.field` or \
                             `param.name`"
                        ),
                    ));
                };
                if binding == "param" {
                    lit.push_str(&env.render(field, path)?);
                    continue;
                }
                let (bind_ref, sort) = if binding == "anchor" {
                    (BindRef::Anchor, anchor_sort)
                } else if Some(binding) == bound_name {
                    (BindRef::Bound, bound_sort.unwrap())
                } else {
                    return Err(PackError::new(
                        path,
                        match bound_name {
                            Some(b) => format!(
                                "unknown binding `{binding}` in template — available: anchor, \
                                 {b}, param"
                            ),
                            None => format!(
                                "unknown binding `{binding}` in template — available: anchor, \
                                 param"
                            ),
                        },
                    ));
                };
                let Some(f) = field_for(sort, field) else {
                    return Err(PackError::new(
                        path,
                        format!(
                            "`{binding}` binds {} which has no field `{field}` — available: {}",
                            sort.describe(),
                            fields_of(sort).join(", ")
                        ),
                    ));
                };
                if !lit.is_empty() {
                    segments.push(Segment::Lit(std::mem::take(&mut lit)));
                }
                segments.push(Segment::Field(bind_ref, f));
            }
            c => lit.push(c),
        }
    }
    if !lit.is_empty() {
        segments.push(Segment::Lit(lit));
    }
    if segments.is_empty() {
        return Err(PackError::new(path, "message template is empty"));
    }
    Ok(segments)
}

/// Validate the whole pack. `raw` is the same JSON the typed `pack` was
/// parsed from (unknown-key checks); `options_by_full_id` maps `pack/rule`
/// to the consumer's options for that rule.
pub(crate) fn validate_pack(
    raw: &serde_json::Value,
    pack: &PackFile,
    options_by_full_id: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
) -> Result<(Vec<ResolvedRule>, Vec<LoadWarning>), PackError> {
    if pack.schema_version != 1 {
        return Err(PackError::new(
            "schemaVersion",
            format!(
                "unsupported schemaVersion {} — only 1 exists",
                pack.schema_version
            ),
        ));
    }
    if pack.name.trim().is_empty() {
        return Err(PackError::new("name", "pack name is empty"));
    }
    if pack.name.contains('/') {
        return Err(PackError::new(
            "name",
            format!("pack name `{}` must not contain `/`", pack.name),
        ));
    }
    if rule_doc(&pack.name).is_some() {
        return Err(PackError::new(
            "name",
            format!(
                "pack name `{}` collides with a built-in diagnostic name",
                pack.name
            ),
        ));
    }

    let mut warnings = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut resolved = Vec::with_capacity(pack.rules.len());
    for (i, def) in pack.rules.iter().enumerate() {
        if !seen.insert(def.id.clone()) {
            return Err(PackError::new(
                format!("rules[{i}].id"),
                format!("duplicate rule id `{}` in this pack", def.id),
            ));
        }
        let full_id = format!("{}/{}", pack.name, def.id);
        let options = options_by_full_id.get(&full_id);
        resolved.push(validate_rule(
            &pack.name,
            i,
            def,
            &raw["rules"][i],
            options.filter(|m| !m.is_empty()),
            &mut warnings,
        )?);
    }
    Ok((resolved, warnings))
}
