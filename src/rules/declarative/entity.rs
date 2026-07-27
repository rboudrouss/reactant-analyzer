//! The entity-edge layer (ADR-022 §1-§2): the adapter between the declarative
//! schema's *entities* and the raw-IR-typed primitives of `rules::api`.
//! Anchors bind rows of engine-resolved relations; edges navigate to typed
//! entities; every name shown to users goes through the native naming
//! discipline (`state_slot_name`, `source_name` — never a bare label).
//!
//! Built once per (rule, component) — the same cost profile as native rules,
//! which recompute these relations per rule. Component-wide relation objects
//! (`ExitDominance`, the conditional-hook proof map) are lazy `OnceCell`s:
//! built on first use by a guard, shared across rows (the
//! relation-object pattern of `setter_in_render`).

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use crate::domains::StateValue;
use crate::engine::{AnalysisResult, EffectInfo, HookCallInfo, HookKind};
use crate::ir::SourceRange;
use crate::ir::expr::Expr;
use crate::ir::free_vars::{AccessPath, dep_paths};
use crate::ir::hooks::HookEntry;
use crate::ir::types::{BlockId, HookLabel, Var};
use crate::rules::api::query::{Certified, ConditionalHookCall, ExitDominance, RuleCtx};
use crate::rules::{
    SetterCall, StabilityVerdict, all_setter_labels, collect_setter_calls, hook_kind_word,
    hook_val_labels, resolve_setter_aliases, state_val_labels,
};

use super::schema::{HookKindFilter, StabilityName};
use super::validate::Field;

// ── Entities ──────────────────────────────────────────────────────────────────

/// One `hook_calls` row, joined to its `HookEntry` (body/init) and
/// `EffectInfo` (deps) by label.
pub(crate) struct HookRow<'a> {
    pub info: &'a HookCallInfo,
    pub entry: Option<&'a HookEntry>,
    pub effect: Option<&'a EffectInfo>,
}

/// An alias-resolved setter call (render or body CFG).
#[derive(Debug, Clone)]
pub(crate) struct SetterEntity {
    pub var: Var,
    pub slot: Option<HookLabel>,
    pub span: Option<SourceRange>,
    pub block_id: Option<BlockId>,
}

/// One declared deps-array entry.
pub(crate) struct DepEntity<'a> {
    pub index: usize,
    pub expr: &'a Expr,
    pub path: Option<AccessPath>,
}

/// A bound entity value, for guards and template rendering.
pub(crate) enum EntityVal<'a, 'b> {
    Hook(&'b HookRow<'a>),
    Setter(&'b SetterEntity),
    Dep(&'b DepEntity<'a>),
}

// ── Per-component index ───────────────────────────────────────────────────────

pub(crate) struct EntityCtx<'a> {
    pub ctx: &'a RuleCtx<'a>,
    pub comp: &'a AnalysisResult<StateValue>,
    /// Canonical alias-resolved setter → slot relation (`all_setter_labels`).
    pub setter_labels: HashMap<Var, HookLabel>,
    pub setter_vars: HashSet<Var>,
    /// State-value bindings in the render CFG (alias-resolved) — the naming
    /// table for slots.
    pub state_names: HashMap<Var, HookLabel>,
    /// `var → label` for every bound hook result, whatever the kind — the
    /// naming table for `{anchor.name}`. Direct bindings only: unlike a slot
    /// name, a hook's name is the variable the call itself binds, not an alias
    /// of it.
    pub hook_names: HashMap<Var, HookLabel>,
    exit_dom: OnceCell<ExitDominance>,
    conditional: OnceCell<HashMap<HookLabel, Certified<ConditionalHookCall>>>,
}

impl<'a> EntityCtx<'a> {
    pub fn new(ctx: &'a RuleCtx<'a>) -> Self {
        let comp = ctx.comp();
        let setter_labels = all_setter_labels(comp);
        let setter_vars = setter_labels.keys().cloned().collect();
        let state_names =
            resolve_setter_aliases(&comp.render_cfg, &state_val_labels(&comp.render_cfg));
        EntityCtx {
            ctx,
            comp,
            setter_labels,
            setter_vars,
            state_names,
            hook_names: hook_val_labels(&comp.render_cfg),
            exit_dom: OnceCell::new(),
            conditional: OnceCell::new(),
        }
    }

    // ── Anchors ───────────────────────────────────────────────────────────────

    /// `hook_calls` rows, kind-filtered, in label order (deterministic).
    pub fn hook_rows(&self, kind: Option<HookKindFilter>) -> Vec<HookRow<'a>> {
        let mut rows: Vec<HookRow<'a>> = self
            .comp
            .hook_calls
            .iter()
            .filter(|info| kind.is_none_or(|k| kind_matches(info.kind, k)))
            .map(|info| HookRow {
                info,
                entry: self.comp.hooks.iter().find(|h| h.label() == info.label),
                effect: self.comp.effect_info.get(&info.label),
            })
            .collect();
        rows.sort_by_key(|r| r.info.label);
        rows
    }

    /// Alias-resolved setter calls in the render body, deterministically
    /// sorted (`collect_setter_calls` returns HashMap order).
    pub fn render_setters(&self) -> Vec<SetterEntity> {
        self.sorted_setters(collect_setter_calls(
            &self.comp.render_cfg,
            &self.setter_vars,
            2,
        ))
    }

    // ── Edges ─────────────────────────────────────────────────────────────────

    /// `body_setter_calls`: setter calls in the anchor's body CFG.
    pub fn body_setters(&self, row: &HookRow<'a>) -> Vec<SetterEntity> {
        let Some(body) = row.entry.and_then(|e| e.body_cfg()) else {
            return vec![];
        };
        self.sorted_setters(collect_setter_calls(body, &self.setter_vars, 2))
    }

    /// `deps`: declared deps-array entries, in declared order. An effect
    /// with no deps array yields an empty list (`has_deps_array` tells them
    /// apart, via the `deps_declared` guard).
    pub fn deps(&self, row: &HookRow<'a>) -> Vec<DepEntity<'a>> {
        let Some(effect) = row.effect else {
            return vec![];
        };
        effect
            .declared_deps
            .iter()
            .enumerate()
            .map(|(index, expr)| DepEntity {
                index,
                expr,
                path: dep_paths(std::slice::from_ref(expr)).into_iter().next(),
            })
            .collect()
    }

    /// Slot labels of the anchor's declared deps (for `in_deps`): a dep's
    /// root variable resolved through the render state-value bindings.
    pub fn dep_slots(&self, row: &HookRow<'a>) -> HashSet<HookLabel> {
        self.deps(row)
            .iter()
            .filter_map(|d| {
                let root = &d.path.as_ref()?.root;
                self.state_names.get(root).copied()
            })
            .collect()
    }

    // ── Component-wide relation objects (lazy) ────────────────────────────────

    pub fn exit_dom(&self) -> &ExitDominance {
        self.exit_dom
            .get_or_init(|| ExitDominance::of(&self.comp.render_cfg))
    }

    pub fn conditional(&self) -> &HashMap<HookLabel, Certified<ConditionalHookCall>> {
        self.conditional.get_or_init(|| {
            self.ctx
                .hook_is_conditional()
                .into_iter()
                .map(|c| (c.evidence().label, c))
                .collect()
        })
    }

    // ── Naming & rendering (the single naming point) ──────────────────────────

    /// Verdict of a deps entry, evaluated at render exit.
    pub fn dep_verdict(&self, dep: &DepEntity<'a>) -> StabilityName {
        verdict_name(&self.ctx.stability_verdict(dep.expr))
    }

    /// The raw value of `field` on `v`: what a text guard matches and what
    /// [`Self::render_field`] decorates — one table, two projections.
    ///
    /// `None` means the entity carries no such value (a slot with no source
    /// name, a hook with no import specifier). Guards **fail** on `None`
    /// (ADR-023: field matching is positive-only, absent ⇒ fail) and rendering
    /// falls back to an anonymous form.
    ///
    /// Total on `Field`, and every arm matches `EntityVal` exhaustively, so a
    /// new field or entity is a compile error here rather than a silently
    /// empty rendering. Combinations the validator rejects answer `None`.
    pub fn field_raw(&self, v: &EntityVal<'a, '_>, field: Field) -> Option<String> {
        match field {
            Field::Kind => match v {
                EntityVal::Hook(h) => Some(hook_kind_word(h.info.kind).to_string()),
                EntityVal::Setter(_) | EntityVal::Dep(_) => None,
            },
            Field::Name => match v {
                // A custom hook is called by its own name; every other kind is
                // called by the variable it binds.
                EntityVal::Hook(h) => match h.entry {
                    Some(HookEntry::Custom { name, .. }) => {
                        Some(crate::ir::source_name(name).to_string())
                    }
                    _ => self.binding_name(h.info.label),
                },
                EntityVal::Setter(_) | EntityVal::Dep(_) => None,
            },
            // The import specifier, and only that: `resolved_file` is an
            // absolute path, so printing or matching it would make a pack's
            // behaviour depend on where the repository sits on disk.
            Field::Source => match v {
                EntityVal::Hook(h) => match h.entry {
                    Some(HookEntry::Custom { import_source, .. }) => import_source.clone(),
                    _ => None,
                },
                EntityVal::Setter(_) | EntityVal::Dep(_) => None,
            },
            Field::Slot => match v {
                EntityVal::Setter(s) => s.slot.and_then(|l| self.slot_source_name(l)),
                EntityVal::Hook(_) | EntityVal::Dep(_) => None,
            },
            Field::Setter => match v {
                EntityVal::Setter(s) => Some(crate::ir::source_name(&s.var).to_string()),
                EntityVal::Hook(_) | EntityVal::Dep(_) => None,
            },
            Field::Path => match v {
                EntityVal::Dep(d) => d.path.as_ref().map(|p| p.to_string()),
                EntityVal::Hook(_) | EntityVal::Setter(_) => None,
            },
            Field::Stability => match v {
                EntityVal::Dep(d) => Some(verdict_word(self.dep_verdict(d)).to_string()),
                EntityVal::Hook(_) | EntityVal::Setter(_) => None,
            },
        }
    }

    /// Render one template field: the raw value, decorated, or the entity's
    /// anonymous form. Never a bare internal label, never an empty string.
    pub fn render_field(&self, v: &EntityVal<'a, '_>, field: Field) -> String {
        let raw = self.field_raw(v, field);
        match field {
            // A missing specifier is not an anonymous entity — it is a hook
            // whose origin we do not know.
            Field::Source => raw.unwrap_or_else(|| "unknown".to_string()),
            Field::Kind | Field::Stability => raw.unwrap_or_else(|| anonymous(v)),
            // Source identifiers are quoted, verdict words are not.
            Field::Name | Field::Slot | Field::Setter | Field::Path => match raw {
                Some(s) => format!("`{s}`"),
                None => anonymous(v),
            },
        }
    }

    /// The unquoted source name of the variable a hook's result binds to.
    fn binding_name(&self, label: HookLabel) -> Option<String> {
        pick_name(&self.hook_names, label)
    }

    /// The unquoted source name of a state slot (alias-aware, like every
    /// native message).
    fn slot_source_name(&self, label: HookLabel) -> Option<String> {
        pick_name(&self.state_names, label)
    }

    fn sorted_setters(&self, calls: Vec<SetterCall>) -> Vec<SetterEntity> {
        let mut setters: Vec<SetterEntity> = calls
            .into_iter()
            .map(|c| SetterEntity {
                slot: self.setter_labels.get(&c.var).copied(),
                var: c.var,
                span: c.span,
                block_id: c.block_id,
            })
            .collect();
        setters.sort_by(|a, b| {
            let pos = |s: &SetterEntity| s.span.map_or((u32::MAX, u32::MAX), |r| r.pos_key());
            (pos(a), &a.var).cmp(&(pos(b), &b.var))
        });
        setters
    }
}

/// How an entity is named when it has no source name. Mirrors the native
/// discipline: an identifier the reader can act on, never a bare label.
fn anonymous(v: &EntityVal<'_, '_>) -> String {
    match v {
        EntityVal::Hook(h) => format!("{} #{}", hook_kind_word(h.info.kind), h.info.label),
        EntityVal::Setter(s) => match s.slot {
            Some(label) => format!("state #{label}"),
            None => format!("`{}`", crate::ir::source_name(&s.var)),
        },
        EntityVal::Dep(d) => format!("dep #{}", d.index),
    }
}

/// The smallest non-temp source name bound to `label`. Smallest, not first:
/// `HashMap` order is seed-dependent and the name is user-visible.
fn pick_name(names: &HashMap<Var, HookLabel>, label: HookLabel) -> Option<String> {
    names
        .iter()
        .filter(|(var, l)| **l == label && !var.starts_with("__"))
        .map(|(var, _)| var)
        .min()
        .map(|var| crate::ir::source_name(var).to_string())
}

fn kind_matches(kind: HookKind, filter: HookKindFilter) -> bool {
    matches!(
        (kind, filter),
        (HookKind::State, HookKindFilter::State)
            | (HookKind::Effect, HookKindFilter::Effect)
            | (HookKind::Memo, HookKindFilter::Memo)
            | (HookKind::Callback, HookKindFilter::Callback)
            | (HookKind::Ref, HookKindFilter::Ref)
            | (HookKind::Custom, HookKindFilter::Custom)
            | (HookKind::Handler, HookKindFilter::Handler)
    )
}

/// Total projection `StabilityVerdict` → verdict name (⊤ stays visible).
pub(crate) fn verdict_name(v: &StabilityVerdict) -> StabilityName {
    match v {
        StabilityVerdict::Stable => StabilityName::Stable,
        StabilityVerdict::Versioned(_) => StabilityName::Versioned,
        StabilityVerdict::PerRender => StabilityName::PerRender,
        StabilityVerdict::Unknown => StabilityName::Unknown,
    }
}

/// `PerRender` is kind-agnostic motion ("changes every render", ADR-017), not
/// a fresh allocation: a `useState` counter converged to a wide interval lands
/// here too. The word must therefore not claim the value is *recreated*.
pub(crate) fn verdict_word(n: StabilityName) -> &'static str {
    match n {
        StabilityName::Stable => "stable",
        StabilityName::Versioned => "versioned",
        StabilityName::PerRender => "changing across renders",
        StabilityName::Unknown => "of unknown stability",
    }
}
