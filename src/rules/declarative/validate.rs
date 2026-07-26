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
    Anchor, EdgeName, ElseBehavior, Guard, HookKindFilter, PVal, PackFile, ParamDecl, ParamType,
    RuleDef, SeverityPin, StabilityName,
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
}

impl Sort {
    fn describe(self) -> String {
        match self {
            Sort::Hook(None) => "a hook call (any kind)".into(),
            Sort::Hook(Some(k)) => format!("a {} hook call", kind_word(k)),
            Sort::SetterRender => "a render-body setter call".into(),
            Sort::SetterBody => "a body setter call".into(),
            Sort::Dep => "a deps entry".into(),
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
    InDeps {
        of: BindRef,
        negate: bool,
    },
    Name {
        of: BindRef,
        one_of: Option<Vec<String>>,
        prefix: Option<String>,
    },
    /// Cardinality of `anchor.deps`.
    Count(CountCmp),
    DepsDeclared {
        eq: bool,
    },
    Must {
        kind: MustKind,
        of: BindRef,
        els: ElseBehavior,
    },
}

/// A template field on a bound entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Kind,
    Name,
    Source,
    Slot,
    Setter,
    Path,
    Stability,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Segment {
    Lit(String),
    Field(BindRef, Field),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolvedAnchor {
    HookCalls(Option<HookKindFilter>),
    RenderSetterCalls,
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
        Guard::InDeps { .. } => ("guard `in_deps`", &["kind", "of", "negate"]),
        Guard::Name { .. } => ("guard `name`", &["kind", "of", "one_of", "prefix"]),
        Guard::Count { .. } => (
            "guard `count`",
            &["kind", "of", "more_than", "less_than", "equals"],
        ),
        Guard::DepsDeclared { .. } => ("guard `deps_declared`", &["kind", "of", "eq"]),
        Guard::MustSetterOnAllPaths { .. } => (
            "guard `must_setter_on_all_paths`",
            &["kind", "of", "else"],
        ),
        Guard::MustDominatesAllExits { .. } => (
            "guard `must_dominates_all_exits`",
            &["kind", "of", "else"],
        ),
        Guard::MustInitCallsSetter { .. } => {
            ("guard `must_init_calls_setter`", &["kind", "of", "else"])
        }
        Guard::MustHookIsConditional { .. } => {
            ("guard `must_hook_is_conditional`", &["kind", "of", "else"])
        }
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
    fn resolve<T: serde::de::DeserializeOwned>(
        &self,
        pv: &PVal<T>,
        expected: Option<ParamType>,
        path: &str,
    ) -> Result<T, PackError>
    where
        T: Clone,
    {
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
            Anchor::RenderSetterCalls => &["relation"],
        },
        "this anchor",
        &format!("{path}.anchor"),
    )?;
    let (anchor, anchor_sort) = match &def.anchor {
        Anchor::HookCalls { kind } => (ResolvedAnchor::HookCalls(*kind), Sort::Hook(*kind)),
        Anchor::RenderSetterCalls => (ResolvedAnchor::RenderSetterCalls, Sort::SetterRender),
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
        };
        bound_sort = Some(element);
        bound_name = Some(fe.bind.as_str());
    }

    // A guard's subject: "anchor" or the forEach binding.
    let resolve_of = |of: &str, g_path: &str| -> Result<(BindRef, Sort), PackError> {
        if of == "anchor" {
            Ok((BindRef::Anchor, anchor_sort))
        } else if Some(of) == bound_name {
            Ok((BindRef::Bound, bound_sort.unwrap()))
        } else {
            Err(PackError::new(
                format!("{g_path}.of"),
                match bound_name {
                    Some(b) => format!("unknown binding `{of}` — available: anchor, {b}"),
                    None => format!("unknown binding `{of}` — available: anchor"),
                },
            ))
        }
    };

    let env = ParamEnv::build(&def.params, options, &path)?;

    // Guards.
    let mut guards = Vec::with_capacity(def.guards.len());
    let mut has_must = false;
    for (gi, guard) in def.guards.iter().enumerate() {
        let g_path = format!("{path}.guards[{gi}]");
        let (what, allowed) = guard_allowed_keys(guard);
        check_keys(&raw_rule["guards"][gi], allowed, what, &g_path)?;

        let resolved = match guard {
            Guard::Stability { of, is, not } => {
                let (of, sort) = resolve_of(of, &g_path)?;
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
                    (Some(pv), None) => (env.resolve(pv, None, &format!("{g_path}.is"))?, false),
                    (None, Some(pv)) => (env.resolve(pv, None, &format!("{g_path}.not"))?, true),
                    _ => {
                        return Err(PackError::new(
                            &g_path,
                            "guard `stability` takes exactly one of `is` / `not`",
                        ));
                    }
                };
                if names.is_empty() {
                    return Err(PackError::new(
                        &g_path,
                        "the verdict list must not be empty",
                    ));
                }
                ResolvedGuard::Stability { of, names, negated }
            }
            Guard::InDeps { of, negate } => {
                let (of, sort) = resolve_of(of, &g_path)?;
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
                if !admits_deps(anchor_sort) {
                    return Err(PackError::new(
                        &g_path,
                        format!(
                            "guard `in_deps` needs an anchor with a deps array \
                             (effect/memo/callback), but the anchor binds {}",
                            anchor_sort.describe()
                        ),
                    ));
                }
                ResolvedGuard::InDeps {
                    of,
                    negate: *negate,
                }
            }
            Guard::Name { of, one_of, prefix } => {
                let (of, sort) = resolve_of(of, &g_path)?;
                let named = matches!(
                    sort,
                    Sort::Hook(Some(HookKindFilter::Custom | HookKindFilter::State))
                        | Sort::SetterBody
                        | Sort::SetterRender
                );
                if !named {
                    return Err(PackError::new(
                        format!("{g_path}.of"),
                        format!(
                            "guard `name` applies to a named entity (custom/state hook, setter \
                             call), but the subject binds {}",
                            sort.describe()
                        ),
                    ));
                }
                let resolved = match (one_of, prefix) {
                    (Some(pv), None) => ResolvedGuard::Name {
                        of,
                        one_of: Some(env.resolve(
                            pv,
                            Some(ParamType::StringList),
                            &format!("{g_path}.one_of"),
                        )?),
                        prefix: None,
                    },
                    (None, Some(pv)) => ResolvedGuard::Name {
                        of,
                        one_of: None,
                        prefix: Some(env.resolve(
                            pv,
                            Some(ParamType::String),
                            &format!("{g_path}.prefix"),
                        )?),
                    },
                    _ => {
                        return Err(PackError::new(
                            &g_path,
                            "guard `name` takes exactly one of `one_of` / `prefix`",
                        ));
                    }
                };
                resolved
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
                if !admits_deps(anchor_sort) {
                    return Err(PackError::new(
                        &g_path,
                        format!(
                            "guard `count` needs an anchor with a deps array \
                             (effect/memo/callback), but the anchor binds {}",
                            anchor_sort.describe()
                        ),
                    ));
                }
                let cmp = match (more_than, less_than, equals) {
                    (Some(pv), None, None) => CountCmp::MoreThan(env.resolve(
                        pv,
                        Some(ParamType::Number),
                        &format!("{g_path}.more_than"),
                    )?),
                    (None, Some(pv), None) => CountCmp::LessThan(env.resolve(
                        pv,
                        Some(ParamType::Number),
                        &format!("{g_path}.less_than"),
                    )?),
                    (None, None, Some(pv)) => CountCmp::Equals(env.resolve(
                        pv,
                        Some(ParamType::Number),
                        &format!("{g_path}.equals"),
                    )?),
                    _ => {
                        return Err(PackError::new(
                            &g_path,
                            "guard `count` takes exactly one of `more_than` / `less_than` / \
                             `equals`",
                        ));
                    }
                };
                ResolvedGuard::Count(cmp)
            }
            Guard::DepsDeclared { of, eq } => {
                let (of_ref, sort) = resolve_of(of, &g_path)?;
                if of_ref != BindRef::Anchor || !admits_deps(sort) {
                    return Err(PackError::new(
                        format!("{g_path}.of"),
                        "guard `deps_declared` applies to an effect/memo/callback anchor",
                    ));
                }
                ResolvedGuard::DepsDeclared {
                    eq: env.resolve(eq, Some(ParamType::Boolean), &format!("{g_path}.eq"))?,
                }
            }
            Guard::MustSetterOnAllPaths { of, r#else } => {
                let (of, sort) = resolve_of(of, &g_path)?;
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
                has_must = true;
                ResolvedGuard::Must {
                    kind: MustKind::SetterOnAllPaths,
                    of,
                    els: *r#else,
                }
            }
            Guard::MustDominatesAllExits { of, r#else } => {
                let (of, sort) = resolve_of(of, &g_path)?;
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
                has_must = true;
                ResolvedGuard::Must {
                    kind: MustKind::DominatesAllExits,
                    of,
                    els: *r#else,
                }
            }
            Guard::MustInitCallsSetter { of, r#else } => {
                let (of_ref, sort) = resolve_of(of, &g_path)?;
                if of_ref != BindRef::Anchor
                    || sort != Sort::Hook(Some(HookKindFilter::State))
                {
                    return Err(PackError::new(
                        format!("{g_path}.of"),
                        "guard `must_init_calls_setter` applies to a state-hook anchor",
                    ));
                }
                has_must = true;
                ResolvedGuard::Must {
                    kind: MustKind::InitCallsSetter,
                    of: of_ref,
                    els: *r#else,
                }
            }
            Guard::MustHookIsConditional { of, r#else } => {
                let (of_ref, sort) = resolve_of(of, &g_path)?;
                if of_ref != BindRef::Anchor || !matches!(sort, Sort::Hook(_)) {
                    return Err(PackError::new(
                        format!("{g_path}.of"),
                        "guard `must_hook_is_conditional` applies to the hook-call anchor",
                    ));
                }
                has_must = true;
                ResolvedGuard::Must {
                    kind: MustKind::HookIsConditional,
                    of: of_ref,
                    els: *r#else,
                }
            }
        };
        guards.push(resolved);
    }

    // Message template.
    let message = parse_template(
        &def.message,
        anchor_sort,
        bound_name,
        bound_sort,
        &env,
        &format!("{path}.message"),
    )?;

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

/// Fields available per sort — the template's type check.
fn field_for(sort: Sort, name: &str) -> Option<Field> {
    match (sort, name) {
        (Sort::Hook(_), "kind") => Some(Field::Kind),
        (Sort::Hook(Some(HookKindFilter::Custom)), "name") => Some(Field::Name),
        (Sort::Hook(Some(HookKindFilter::Custom)), "source") => Some(Field::Source),
        (Sort::Hook(Some(HookKindFilter::State)), "name") => Some(Field::Name),
        (Sort::SetterBody | Sort::SetterRender, "slot") => Some(Field::Slot),
        (Sort::SetterBody | Sort::SetterRender, "setter") => Some(Field::Setter),
        (Sort::Dep, "path") => Some(Field::Path),
        (Sort::Dep, "stability") => Some(Field::Stability),
        _ => None,
    }
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
                            "`{binding}` binds {} which has no field `{field}`",
                            sort.describe()
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
            format!("unsupported schemaVersion {} — only 1 exists", pack.schema_version),
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
        let options = options_by_full_id.get(&full_id).map(|m| {
            // Only pass through when non-empty: an empty options object is
            // indistinguishable from no options.
            m
        });
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
