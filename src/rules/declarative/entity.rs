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
    resolve_setter_aliases, state_slot_name, state_val_labels,
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

    pub fn slot_display(&self, label: HookLabel) -> String {
        state_slot_name(label, &self.state_names)
    }

    /// The raw (unquoted) name of an entity, for `name` guard matching:
    /// custom hook name, state slot's source variable, or setter variable.
    pub fn raw_name(&self, v: &EntityVal<'a, '_>) -> Option<String> {
        match v {
            EntityVal::Hook(h) => match h.entry {
                Some(HookEntry::Custom { name, .. }) => {
                    Some(crate::ir::source_name(name).to_string())
                }
                Some(HookEntry::State { .. }) => self
                    .state_names
                    .iter()
                    .find(|(var, l)| **l == h.info.label && !var.starts_with("__"))
                    .map(|(var, _)| crate::ir::source_name(var).to_string()),
                _ => None,
            },
            EntityVal::Setter(s) => Some(crate::ir::source_name(&s.var).to_string()),
            EntityVal::Dep(d) => d.path.as_ref().map(|p| p.to_string()),
        }
    }

    /// Verdict of a deps entry, evaluated at render exit.
    pub fn dep_verdict(&self, dep: &DepEntity<'a>) -> StabilityName {
        verdict_name(&self.ctx.stability_verdict(dep.expr))
    }

    /// Render one template field against a bound entity.
    pub fn render_field(&self, v: &EntityVal<'a, '_>, field: Field) -> String {
        match (v, field) {
            (EntityVal::Hook(h), Field::Kind) => hook_kind_word(h.info.kind).to_string(),
            (EntityVal::Hook(h), Field::Name) => match h.entry {
                Some(HookEntry::Custom { name, .. }) => {
                    format!("`{}`", crate::ir::source_name(name))
                }
                _ => self.slot_display(h.info.label),
            },
            (EntityVal::Hook(h), Field::Source) => match h.entry {
                Some(HookEntry::Custom {
                    import_source: Some(src),
                    ..
                }) => src.clone(),
                Some(HookEntry::Custom {
                    resolved_file: Some(f),
                    ..
                }) => f.display().to_string(),
                _ => "unknown".to_string(),
            },
            (EntityVal::Setter(s), Field::Slot) => match s.slot {
                Some(label) => self.slot_display(label),
                None => format!("`{}`", crate::ir::source_name(&s.var)),
            },
            (EntityVal::Setter(s), Field::Setter) => {
                format!("`{}`", crate::ir::source_name(&s.var))
            }
            (EntityVal::Dep(d), Field::Path) => match &d.path {
                Some(p) => format!("`{p}`"),
                None => format!("dep #{}", d.index),
            },
            (EntityVal::Dep(d), Field::Stability) => verdict_word(self.dep_verdict(d)).to_string(),
            // Unreachable by validation (field_for), but total anyway.
            _ => String::new(),
        }
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
