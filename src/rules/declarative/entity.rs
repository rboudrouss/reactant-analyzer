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
use crate::engine::setters::WriteProvenance;
use crate::engine::{
    AnalysisResult, EffectInfo, HookCallInfo, HookKind, SeedSync, SlotSeed, SlotWriter,
    WriterPhase, WriterRegion,
};
use crate::ir::SourceRange;
use crate::ir::expr::Expr;
use crate::ir::free_vars::{AccessPath, dep_paths};
use crate::ir::hooks::{HookEntry, HookProvenance};
use crate::ir::types::{BlockId, HookLabel, Symbol, Var};
use crate::rules::api::query::{
    Certified, CleanupVerdict, ConditionalHookCall, ExitDominance, RuleCtx,
};
use crate::rules::helpers::churn_graph::{CycleRow, collect_cycle_rows};
use crate::rules::helpers::context_flow::{ConsumerRow, ProviderVerdict};
use crate::rules::helpers::jsx::{JsxPropSite, collect_jsx_prop_sites, site_identity};
use crate::rules::helpers::local_bindings;
use crate::rules::helpers::providers::{ProviderSite, ValueIdentity, collect_provider_sites};
use crate::rules::{
    ReturnsVerdict, SetterCall, StabilityVerdict, all_setter_labels, collect_setter_calls,
    cross_component_setters, hook_kind_word, hook_val_labels, resolve_setter_aliases,
    state_val_labels,
};

use super::schema::{
    HookKindFilter, ImpureName, PhaseName, ProviderName, ReturnsName, SeedSyncName, StabilityName,
    UpdaterName,
};
use super::validate::Field;
use crate::rules::helpers::purity::{ImpureBody, classify_body};

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
    /// Which component owns the slot this call writes (#107). `None` for a
    /// local setter — the anchored component owns it; `Some(parent)` for a
    /// `ComponentSetter`-valued prop the top-down pass placed here.
    ///
    /// A foreign row's `slot` is a label of the OWNER's component, so it must
    /// never be resolved against this component's naming table: labels are
    /// per-component and would collide.
    pub owner: Option<Symbol>,
}

/// One declared deps-array entry.
pub(crate) struct DepEntity<'a> {
    pub expr: &'a Expr,
    pub path: Option<AccessPath>,
}

/// One call-site argument of a custom-hook anchor (ADR-023 §3). Carries the
/// hook label so the `returns` guard can read the fixpoint-computed verdict.
pub(crate) struct ArgEntity {
    pub label: HookLabel,
    pub index: usize,
}

/// A bound entity value, for guards and template rendering.
pub(crate) enum EntityVal<'a, 'b> {
    Hook(&'b HookRow<'a>),
    Setter(&'b SetterEntity),
    Dep(&'b DepEntity<'a>),
    Arg(&'b ArgEntity),
    /// One `hook_origins` row (ADR-027 §7).
    Origin(&'a HookProvenance),
    /// One writer of a state slot (ADR-027 §1).
    Writer(&'a SlotWriter),
    /// One proven context-provider element (#71).
    Provider(&'b ProviderSite<'a>),
    /// One prop of one resolved component element (#71 step 2).
    JsxProp(&'b JsxPropSite<'a>),
    /// One render-loop cycle carried by this component's effects (#108).
    Cycle(&'b CycleRow),
    /// One prop seed of a state-hook anchor's slot (#106).
    Seed(&'a SlotSeed),
    /// One `useContext` call site with complete ancestry (#115).
    Consumer(&'b ConsumerRow),
    /// One callback registration in an effect body (#111).
    Registration(&'a crate::engine::registrations::Registration),
    /// One non-hook call site in a body (#126).
    Call(&'b crate::engine::BodyCall),
    /// One read site of a state slot (#127).
    Read(&'b crate::engine::SlotRead),
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
    /// Index into `comp.hook_provenance` by label (indices, not references:
    /// `OnceCell` is invariant, so a borrowed map would freeze `'a`).
    provenance: OnceCell<HashMap<HookLabel, usize>>,
    /// `ComponentSetter`-valued props (#107), resolved on first use by the
    /// ownership-aware enumeration.
    cross_setters: OnceCell<HashMap<Var, (Symbol, HookLabel)>>,
    /// The slot → readers relation (#127), computed on first use: unlike the
    /// writers, it is not needed by any native rule, so a component no pack
    /// asks about never walks for it.
    slot_reads: OnceCell<Vec<crate::engine::SlotRead>>,
    /// Body calls per hook (#126), all bodies at once on first use. A rule
    /// that navigates `calls` and then asks `none of anchor.calls` would
    /// otherwise walk the body once per row — quadratic in the body's size.
    body_calls: OnceCell<HashMap<HookLabel, Vec<crate::engine::BodyCall>>>,
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
            provenance: OnceCell::new(),
            cross_setters: OnceCell::new(),
            slot_reads: OnceCell::new(),
            body_calls: OnceCell::new(),
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
    /// `foreign` widens the enumeration with `ComponentSetter`-valued props —
    /// parent setters the top-down pass placed in this component's environment
    /// (#107).
    ///
    /// **Never widened unconditionally.** The validator sets the flag only for
    /// a rule that names ownership, so a pack shipped before this existed
    /// enumerates exactly the rows it enumerated then: changing what a shipped
    /// sort binds changes which findings fire (the ADR-027 §2 sequencing
    /// argument).
    pub fn render_setters(&self, foreign: bool) -> Vec<SetterEntity> {
        if !foreign {
            return self.sorted_setters(
                collect_setter_calls(&self.comp.render_cfg, &self.setter_vars, 2),
                &HashMap::new(),
            );
        }
        let cross = self.cross_setters();
        let mut vars = self.setter_vars.clone();
        vars.extend(cross.keys().cloned());
        self.sorted_setters(collect_setter_calls(&self.comp.render_cfg, &vars, 2), cross)
    }

    /// `var → (owning component, slot)` for every `ComponentSetter`-valued
    /// prop of this component — the engine resolution the native
    /// `setter-in-render` rule consumes, read once and shared (ADR-027 §1).
    fn cross_setters(&self) -> &HashMap<Var, (Symbol, HookLabel)> {
        self.cross_setters
            .get_or_init(|| cross_component_setters(self.comp, self.ctx.component()))
    }

    /// `hook_origins` rows in label order (labels are unique after the
    /// offset merge; the origin name breaks ties defensively so the order
    /// stays total either way).
    pub fn origin_rows(&self) -> Vec<&'a HookProvenance> {
        let mut rows: Vec<&'a HookProvenance> = self.comp.hook_provenance.iter().collect();
        rows.sort_by(|a, b| (a.label, &a.origin_hook).cmp(&(b.label, &b.origin_hook)));
        rows
    }

    /// `context_providers` rows: every proven provider element in the render
    /// body, deterministic order (the relation sorts by site).
    pub fn provider_rows(&self) -> Vec<ProviderSite<'a>> {
        collect_provider_sites(self.comp)
    }

    /// Identity of a custom-hook call-site argument, read at the call's OWN
    /// program point (#112).
    ///
    /// ADR-023 §2 forbids reading render-EXIT stability here, because exit is a
    /// different point: `let x = {}; useThing(x); x = props.stable` is Stable
    /// at exit and fresh at the call. This is §2's own escape rather than a
    /// bypass — the expression is evaluated in the converged env of the block
    /// the call sits in, and the shared bind-once rule collapses that very
    /// counterexample (two bindings) to Unknown instead of answering it wrong.
    pub fn arg_identity(&self, arg: &ArgEntity) -> ValueIdentity {
        let Some(HookEntry::Custom { args, .. }) =
            self.comp.hooks.iter().find(|h| h.label() == arg.label)
        else {
            return ValueIdentity::Unknown;
        };
        let Some(expr) = args.get(arg.index) else {
            return ValueIdentity::Unknown;
        };
        let Some(info) = self.comp.hook_calls.iter().find(|i| i.label == arg.label) else {
            return ValueIdentity::Unknown;
        };
        site_identity(
            Some(expr),
            self.comp.block_states.get(&info.block_id),
            &local_bindings(&self.comp.render_cfg),
            self.comp,
        )
    }

    /// `registrations` rows (#111): the engine relation, already computed at
    /// convergence, filtered to nothing — every row belongs to this component.
    pub fn registration_rows(&self) -> &'a [crate::engine::registrations::Registration] {
        &self.comp.registrations
    }

    /// Is a registration's listener a fresh reference on every run of the
    /// effect that registers it (#116)?
    ///
    /// An inline literal is fresh by construction — the allocation site is the
    /// registration itself. A name goes through the same `site_identity`
    /// reader the two JSX relations use, evaluated in the effect body's own
    /// block env, so the bind-once rule answers Unknown for a name that could
    /// mean two things. Anything else is Unknown, and Unknown never fires.
    pub fn listener_identity(
        &self,
        row: &crate::engine::registrations::Registration,
    ) -> ValueIdentity {
        let Some(HookEntry::Effect { body_cfg, .. }) =
            self.comp.hooks.iter().find(|h| h.label() == row.effect)
        else {
            return ValueIdentity::Unknown;
        };
        match row.callback.peel_ts() {
            Expr::FnLit { .. } => ValueIdentity::FreshEveryRender,
            Expr::Var(_) => site_identity(
                Some(&row.callback),
                row.block_id.and_then(|b| {
                    self.comp
                        .effect_block_states
                        .get(&row.effect)
                        .and_then(|blocks| blocks.get(&b))
                }),
                &local_bindings(body_cfg),
                self.comp,
            ),
            _ => ValueIdentity::Unknown,
        }
    }

    /// `jsx_props` rows: every prop of every element of the requested kinds in
    /// the render body, deterministic order (the relation sorts by site).
    pub fn jsx_prop_rows(
        &self,
        kinds: crate::rules::helpers::jsx::ElementKinds,
    ) -> Vec<JsxPropSite<'a>> {
        collect_jsx_prop_sites(self.comp, kinds)
    }

    /// `context_consumers` rows (#115): this component's `useContext` calls
    /// whose ancestry the analysis could complete.
    ///
    /// Read from the [`crate::rules::api::cache::ProgramCache`], so the
    /// relation is built once per program — its verdict depends on every
    /// component that may render this one, which makes it whole-program data
    /// for the same reason the churn graph is (#86).
    pub fn consumer_rows(&self) -> Vec<&'a ConsumerRow> {
        // The cache outlives `self`, so the rows borrow for `'a`.
        let consumers: &'a _ = self.ctx.cache().context_consumers();
        consumers.of(self.ctx.component())
    }

    /// `churn_cycles` rows (#108): the program graph's render loops, projected
    /// onto the effects of THIS component that carry one of their edges.
    ///
    /// The graph comes from the [`crate::rules::api::cache::ProgramCache`], so
    /// it is built once per program and shared with the native `infinite-loop`
    /// arm that reads it — never rebuilt per rule or per component (#86).
    pub fn cycle_rows(&self) -> Vec<CycleRow> {
        collect_cycle_rows(
            self.ctx.cache().churn(),
            self.ctx.program(),
            self.ctx.component(),
        )
    }

    // ── Edges ─────────────────────────────────────────────────────────────────

    /// `writers`: the anchor slot's rows of the slot-writer relation, in the
    /// relation's (already deterministic) order.
    pub fn writers(&self, row: &HookRow<'a>) -> Vec<&'a SlotWriter> {
        self.comp
            .slot_writers
            .iter()
            .filter(|w| w.slot == row.info.label)
            .collect()
    }

    /// The anchor effect's teardown verdict (#100). The one shared reader of
    /// `query::cleanup_verdict`, which the native missing-cleanup rule calls
    /// too — the fact is computed in one place, never mirrored (ADR-027 §1).
    /// A row with no body CFG answers `Unknown`, the may side, so the only
    /// actionable verdict (`Absent`) is never claimed for a body we cannot see.
    pub fn cleanup(&self, row: &HookRow<'a>) -> CleanupVerdict {
        row.entry.and_then(|e| e.body_cfg()).map_or(
            CleanupVerdict::Unknown,
            crate::rules::api::query::cleanup_verdict,
        )
    }

    /// `body_setter_calls`: setter calls in the anchor's body CFG.
    pub fn body_setters(&self, row: &HookRow<'a>) -> Vec<SetterEntity> {
        let Some(body) = row.entry.and_then(|e| e.body_cfg()) else {
            return vec![];
        };
        self.sorted_setters(
            collect_setter_calls(body, &self.setter_vars, 2),
            &HashMap::new(),
        )
    }

    /// `calls`: non-hook call sites in the anchor's body CFG (#126, ADR-036).
    ///
    /// The same depth budget the `body_setter_calls` edge uses, and the same
    /// walk — the phase of a call in a `.then`, past an `await`, or in the
    /// effect's returned cleanup is the walk's answer, not a second one.
    pub fn body_calls(&self, row: &HookRow<'a>) -> &[crate::engine::BodyCall] {
        self.body_calls
            .get_or_init(|| {
                self.comp
                    .hook_calls
                    .iter()
                    .filter_map(|info| {
                        let entry = self.comp.hooks.iter().find(|h| h.label() == info.label)?;
                        let body = entry.body_cfg()?;
                        let row = HookRow {
                            info,
                            entry: Some(entry),
                            effect: None,
                        };
                        Some((
                            info.label,
                            crate::engine::collect_body_calls(body, region_of(&row), 2),
                        ))
                    })
                    .collect()
            })
            .get(&row.info.label)
            .map_or(&[][..], Vec::as_slice)
    }

    /// `render_calls`: non-hook call sites in the render body (#126).
    pub fn render_call_rows(&self) -> Vec<crate::engine::BodyCall> {
        crate::engine::collect_body_calls(&self.comp.render_cfg, WriterRegion::Render, 2)
    }

    /// `deps`: declared deps-array entries, in declared order. An effect with
    /// no readable deps array yields an empty list (the `deps_declared` guard
    /// tells that apart from a written `[]`), and so does one whose entries the
    /// lowering could not keep one-for-one — enumerating what is there stays
    /// sound, only counting it does not (the `count` guard refuses).
    pub fn deps(&self, row: &HookRow<'a>) -> Vec<DepEntity<'a>> {
        let Some(effect) = row.effect else {
            return vec![];
        };
        effect
            .declared_deps()
            .iter()
            .map(|expr| DepEntity {
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

    /// `reads`: the anchor slot's read sites (#127), in the relation's own
    /// deterministic order.
    pub fn reads(&self, row: &HookRow<'a>) -> Vec<&crate::engine::SlotRead> {
        self.slot_reads
            .get_or_init(|| {
                crate::engine::collect_slot_reads(&self.comp.render_cfg, &self.comp.hooks)
            })
            .iter()
            .filter(|r| r.slot == row.info.label)
            .collect()
    }

    /// `seeds`: the anchor slot's prop seeds, in the relation's (already
    /// deterministic) order. Computed at convergence and stored on the
    /// component, so the native `frozen-initial-state` rule and this edge read
    /// one relation (#106, ADR-031).
    pub fn seeds(&self, row: &HookRow<'a>) -> Vec<&'a SlotSeed> {
        self.comp.seeds_of(row.info.label).collect()
    }

    /// `args`: call-site arguments of a custom-hook anchor, in call order.
    /// Only the anchor's row identity travels — the verdict of what a
    /// function-valued argument returns was computed during the fixpoint
    /// (`AnalysisResult::custom_arg_returns`) and is read per (label, index).
    pub fn args(&self, row: &HookRow<'a>) -> Vec<ArgEntity> {
        let Some(HookEntry::Custom { args, .. }) = row.entry else {
            return vec![];
        };
        (0..args.len())
            .map(|index| ArgEntity {
                label: row.info.label,
                index,
            })
            .collect()
    }

    // ── Component-wide relation objects (lazy) ────────────────────────────────

    pub fn exit_dom(&self) -> &ExitDominance {
        self.exit_dom
            .get_or_init(|| ExitDominance::of(&self.comp.render_cfg))
    }

    /// Provenance row of a hook call (ADR-023 step 1), by label.
    pub fn provenance(&self, label: HookLabel) -> Option<&'a crate::ir::hooks::HookProvenance> {
        let idx = self.provenance.get_or_init(|| {
            self.comp
                .hook_provenance
                .iter()
                .enumerate()
                .map(|(i, p)| (p.label, i))
                .collect()
        });
        idx.get(&label).map(|&i| &self.comp.hook_provenance[i])
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

    /// `writer_phases includes` (ADR-027 §1): does some write of `label` MAY
    /// run in one of the named phases? A ⊤ row satisfies every query.
    pub fn writer_phase_includes(&self, label: HookLabel, names: &[PhaseName]) -> bool {
        self.comp
            .slot_writers
            .iter()
            .filter(|w| w.slot == label)
            .any(|w| w.phase == WriterPhase::Unknown || names.contains(&phase_name(w.phase)))
    }

    /// Returns-verdict of a call-site argument (⊤-total, ADR-023 §3).
    pub fn arg_verdict(&self, arg: &ArgEntity) -> ReturnsName {
        returns_name(self.ctx.returns_verdict(arg.label, arg.index))
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
                EntityVal::JsxProp(j) => {
                    Some(if j.host { "host" } else { "component" }.to_string())
                }
                EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
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
                // The origin hook's own name — the resolved identity, never a
                // binding variable (there may be none: the row survives
                // inlining). A provider's name is its context binding.
                EntityVal::Origin(p) => Some(p.origin_hook.clone()),
                EntityVal::Provider(p) => Some(crate::ir::source_name(p.context).to_string()),
                EntityVal::JsxProp(j) => Some(j.element.to_string()),
                // The local binding the call reads the context through.
                EntityVal::Consumer(c) => Some(crate::ir::source_name(&c.name).to_string()),
                // A registration is named by its registrar — the table key,
                // matchable by a `name` guard. The receiver-qualified form
                // (`socket.addEventListener`) stays internal to the native
                // rules' messages: a pack cannot match what varies per site.
                EntityVal::Registration(r) => Some(r.registrar.to_string()),
                // A call row is named by its callee: the function for a bare
                // call, the method for a member call. The receiver is the
                // separate `receiver` field — one fact per field, so a rule
                // that wants `socket.join` writes two guards and one that
                // wants any `.join` writes one.
                EntityVal::Call(c) => Some(c.name.clone()),
                // A read row is named by the binding it went through — the
                // slot's own name, or the alias the site actually wrote.
                EntityVal::Read(r) => Some(crate::ir::source_name(&r.name).to_string()),
                EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Writer(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_) => None,
            },
            // The root binding of a member call's receiver; `None` for a bare
            // call, which is an absence and so fails a guard rather than
            // matching an empty string.
            Field::Receiver => match v {
                EntityVal::Read(_) => None,
                EntityVal::Call(c) => c.receiver.clone(),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_) => None,
            },
            // The import specifier, and only that: `resolved_file` is an
            // absolute path, so printing or matching it would make a pack's
            // behaviour depend on where the repository sits on disk.
            Field::Source => match v {
                EntityVal::Hook(h) => match h.entry {
                    Some(HookEntry::Custom { import_source, .. }) => import_source.clone(),
                    _ => None,
                },
                EntityVal::Origin(p) => p.specifier.clone(),
                EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            Field::Slot => match v {
                EntityVal::Read(r) => self.slot_source_name(r.slot),
                EntityVal::Setter(s) => self.setter_slot_name(s),
                EntityVal::Writer(w) => self.slot_source_name(w.slot),
                EntityVal::Hook(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_) => None,
            },
            Field::Setter => match v {
                EntityVal::Setter(s) => Some(crate::ir::source_name(&s.var).to_string()),
                EntityVal::Writer(w) => Some(crate::ir::source_name(&w.setter).to_string()),
                EntityVal::Hook(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            // A seed row IS a prop path, and so is a deps entry.
            Field::Path => match v {
                EntityVal::Dep(d) => d.path.as_ref().map(|p| p.to_string()),
                EntityVal::Seed(s) => Some(s.path.to_string()),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            Field::Stability => match v {
                EntityVal::Dep(d) => Some(verdict_word(self.dep_verdict(d)).to_string()),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            Field::Returns => match v {
                EntityVal::Arg(a) => Some(returns_word(self.arg_verdict(a)).to_string()),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            Field::Region => match v {
                EntityVal::Read(r) => Some(r.region.word().to_string()),
                EntityVal::Writer(w) => Some(w.region.word().to_string()),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_) => None,
            },
            Field::Phase => match v {
                EntityVal::Writer(w) => Some(phase_word(w.phase).to_string()),
                EntityVal::Call(c) => Some(phase_word(c.phase).to_string()),
                EntityVal::Read(r) => Some(phase_word(r.phase).to_string()),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_) => None,
            },
            Field::Via => match v {
                EntityVal::Writer(w) => Some(match &w.via {
                    WriteProvenance::Direct => "direct".to_string(),
                    WriteProvenance::Via(chain) => chain
                        .iter()
                        .map(|n| crate::ir::source_name(n).to_string())
                        .collect::<Vec<_>>()
                        .join(" → "),
                    WriteProvenance::Unknown => "unknown".to_string(),
                }),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            Field::Identity => match v {
                EntityVal::Read(_) => None,
                EntityVal::Call(_) => None,
                EntityVal::Provider(p) => Some(identity_word(p.identity).to_string()),
                EntityVal::JsxProp(j) => Some(identity_word(j.identity).to_string()),
                EntityVal::Arg(a) => Some(identity_word(self.arg_identity(a)).to_string()),
                // The listener's verdict, through the same `site_identity`
                // reader the two JSX relations use (#116).
                EntityVal::Registration(r) => {
                    Some(identity_word(self.listener_identity(r)).to_string())
                }
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_) => None,
            },
            Field::Prop => match v {
                EntityVal::JsxProp(j) => Some(j.prop.to_string()),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            Field::Cleanup => match v {
                EntityVal::Hook(row) => Some(cleanup_word(self.cleanup(row)).to_string()),
                EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            // Which component owns the slot the row writes: this one for a
            // local setter, the parent for a `ComponentSetter`-valued prop.
            Field::Owner => match v {
                EntityVal::Setter(s) => Some(
                    s.owner
                        .clone()
                        .unwrap_or_else(|| self.ctx.component().clone()),
                ),
                EntityVal::Hook(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            // The loop path: node names are already qualified and quoted by
            // the shared `node_display`, so this one is rendered bare.
            Field::Cycle => match v {
                EntityVal::Cycle(c) => Some(c.path.clone()),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_)
                | EntityVal::Registration(_)
                | EntityVal::Call(_)
                | EntityVal::Read(_) => None,
            },
            Field::Firing => match v {
                EntityVal::Read(_) => None,
                EntityVal::Call(_) => None,
                EntityVal::Registration(r) => Some(
                    match r.firing {
                        crate::engine::registrations::Firing::Repeating => "repeating",
                        crate::engine::registrations::Firing::Once => "once",
                    }
                    .to_string(),
                ),
                EntityVal::Hook(_)
                | EntityVal::Setter(_)
                | EntityVal::Dep(_)
                | EntityVal::Arg(_)
                | EntityVal::Origin(_)
                | EntityVal::Writer(_)
                | EntityVal::Provider(_)
                | EntityVal::JsxProp(_)
                | EntityVal::Cycle(_)
                | EntityVal::Seed(_)
                | EntityVal::Consumer(_) => None,
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
            // And a bare call has no receiver at all — not an anonymous one.
            Field::Receiver => raw.map_or_else(|| "no receiver".to_string(), |s| format!("`{s}`")),
            Field::Kind
            | Field::Stability
            | Field::Returns
            | Field::Region
            | Field::Phase
            | Field::Via
            | Field::Identity
            | Field::Cleanup
            | Field::Firing
            | Field::Cycle => raw.unwrap_or_else(|| anonymous(v)),
            // Source identifiers are quoted, verdict words are not.
            Field::Name
            | Field::Slot
            | Field::Setter
            | Field::Path
            | Field::Prop
            | Field::Owner => match raw {
                Some(s) => format!("`{s}`"),
                None => anonymous(v),
            },
        }
    }

    /// The unquoted source name of the slot a setter row writes.
    ///
    /// A foreign row's label belongs to the OWNER's component, so it is
    /// resolved there. Resolving it against this component's table would name
    /// an unrelated local slot that happens to share the label — labels are
    /// per-component.
    fn setter_slot_name(&self, s: &SetterEntity) -> Option<String> {
        let label = s.slot?;
        let Some(owner) = &s.owner else {
            return self.slot_source_name(label);
        };
        let comp = self.ctx.program().components.get(owner)?;
        let names = resolve_setter_aliases(&comp.render_cfg, &state_val_labels(&comp.render_cfg));
        pick_name(&names, label)
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

    fn sorted_setters(
        &self,
        calls: Vec<SetterCall>,
        cross: &HashMap<Var, (Symbol, HookLabel)>,
    ) -> Vec<SetterEntity> {
        let mut setters: Vec<SetterEntity> = calls
            .into_iter()
            .map(|c| {
                // A local binding wins: a component passing its own setter down
                // is not a foreign write, and `cross_component_setters` already
                // filtered self-owned entries out.
                let (slot, owner) = match self.setter_labels.get(&c.var) {
                    Some(&label) => (Some(label), None),
                    None => match cross.get(&c.var) {
                        Some((comp, label)) => (Some(*label), Some(comp.clone())),
                        None => (None, None),
                    },
                };
                SetterEntity {
                    slot,
                    owner,
                    var: c.var,
                    span: c.span,
                    block_id: c.block_id,
                }
            })
            .collect();
        setters.sort_by(|a, b| {
            let pos = |s: &SetterEntity| s.span.map_or((u32::MAX, u32::MAX), |r| r.pos_key());
            (pos(a), &a.var).cmp(&(pos(b), &b.var))
        });
        setters
    }
}

/// The lexical region a body anchor's calls sit in — what decides the phase
/// word a synchronous call gets. Kind-pinned by the validator to the four
/// anchors with a body, so the fallback is unreachable in practice and stays
/// on the ⊤ side if it ever is not.
fn region_of(row: &HookRow<'_>) -> WriterRegion {
    match row.info.kind {
        HookKind::Effect => WriterRegion::Effect(row.info.label),
        HookKind::Memo => WriterRegion::Memo(row.info.label),
        HookKind::Callback => WriterRegion::Callback(row.info.label),
        _ => WriterRegion::Handler(row.info.label),
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
        // The dependency as written, never its position: an index into `elems`
        // stops being an index into the source array once lowering drops an
        // elision or flattens a spread (#118).
        EntityVal::Dep(d) => format!("`{}`", d.expr.describe()),
        EntityVal::Arg(a) => format!("argument #{}", a.index),
        EntityVal::Origin(p) => format!("`{}`", p.origin_hook),
        EntityVal::Writer(w) => format!("`{}`", crate::ir::source_name(&w.setter)),
        EntityVal::Provider(p) => format!("`{}.Provider`", crate::ir::source_name(p.context)),
        EntityVal::JsxProp(j) => format!("`{}` of `<{}>`", j.prop, j.element),
        EntityVal::Cycle(c) => format!("the loop through effect #{}", c.effect),
        EntityVal::Read(r) => format!("`{}`", crate::ir::source_name(&r.name)),
        EntityVal::Call(c) => match &c.receiver {
            Some(r) => format!("`{}.{}()`", r, c.name),
            None => format!("`{}()`", c.name),
        },
        EntityVal::Seed(s) => format!("`{}`", s.path),
        EntityVal::Consumer(c) => format!("`{}`", crate::ir::source_name(&c.name)),
        EntityVal::Registration(r) => format!("`{}`", r.display),
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

/// `WriterPhase` → schema name (total — a new phase is a compile error here).
pub(crate) fn phase_name(p: WriterPhase) -> PhaseName {
    match p {
        WriterPhase::Render => PhaseName::Render,
        WriterPhase::Effect => PhaseName::Effect,
        WriterPhase::Memo => PhaseName::Memo,
        WriterPhase::Callback => PhaseName::Callback,
        WriterPhase::Handler => PhaseName::Handler,
        WriterPhase::Deferred => PhaseName::Deferred,
        WriterPhase::Cleanup => PhaseName::Cleanup,
        WriterPhase::Unknown => PhaseName::Unknown,
    }
}

/// `CleanupVerdict` → schema name (total).
pub(crate) fn cleanup_name(c: CleanupVerdict) -> super::schema::CleanupName {
    match c {
        CleanupVerdict::Present => super::schema::CleanupName::Present,
        CleanupVerdict::Absent => super::schema::CleanupName::Absent,
        CleanupVerdict::Unknown => super::schema::CleanupName::Unknown,
    }
}

/// The word rendered by `{anchor.cleanup}`.
fn cleanup_word(c: CleanupVerdict) -> &'static str {
    match c {
        CleanupVerdict::Present => "present",
        CleanupVerdict::Absent => "absent",
        CleanupVerdict::Unknown => "unknown",
    }
}

/// `ValueIdentity` → schema name (total).
/// The `updater_body` guard's total mirror: is the recorded updater a function
/// literal whose body writes something it does not own?
///
/// Reads the same column [`updater_name`] classifies — one recorded
/// expression, two derived verdicts (ADR-028 §2). An updater the walk could
/// not resolve to a literal has no body to classify and answers ⊤, so the
/// unresolved case never fires.
impl<'a> EntityCtx<'a> {
    pub fn updater_purity(&self, u: &crate::engine::setters::Updater) -> ImpureName {
        use crate::engine::setters::Updater;
        let Updater::Functional(body) = u else {
            return ImpureName::Unknown;
        };
        match classify_body(body, &self.setter_vars) {
            ImpureBody::Impure => ImpureName::Impure,
            ImpureBody::Unknown => ImpureName::Unknown,
        }
    }
}

/// Total mirror of the provider verdict (#115).
pub(crate) fn provider_name(v: ProviderVerdict) -> ProviderName {
    match v {
        ProviderVerdict::ProviderSeen => ProviderName::ProviderSeen,
        ProviderVerdict::NoneOnAnalyzedPaths => ProviderName::NoneOnAnalyzedPaths,
    }
}

/// Total mirror of the seed-sync verdict (#106).
pub(crate) fn seed_sync_name(s: SeedSync) -> SeedSyncName {
    match s {
        SeedSync::Synced => SeedSyncName::Synced,
        SeedSync::NoneSeen => SeedSyncName::NoneSeen,
    }
}

/// The `teardown` guard's mirror of the pairing fact (#111). Three engine
/// values collapse to two schema names: the unresolvable case is an absence of
/// evidence, and the vocabulary says so — `none-seen`, never `unpaired`.
pub(crate) fn teardown_name(
    p: crate::engine::registrations::Pairing,
) -> super::schema::TeardownName {
    match p {
        crate::engine::registrations::Pairing::Paired => super::schema::TeardownName::Paired,
        crate::engine::registrations::Pairing::Unpaired
        | crate::engine::registrations::Pairing::Unknown => super::schema::TeardownName::NoneSeen,
    }
}

/// Total mirror of a registration's firing class (#111).
pub(crate) fn firing_name(f: crate::engine::registrations::Firing) -> super::schema::FiringName {
    match f {
        crate::engine::registrations::Firing::Repeating => super::schema::FiringName::Repeating,
        crate::engine::registrations::Firing::Once => super::schema::FiringName::Once,
    }
}

/// The `updater` guard's total mirror of a `writers` row's argument-0 column
/// (ADR-028 §2): only a proven function literal is `functional`.
pub(crate) fn updater_name(u: &crate::engine::setters::Updater) -> UpdaterName {
    if u.is_functional() {
        UpdaterName::Functional
    } else {
        UpdaterName::Unknown
    }
}

pub(crate) fn identity_name(i: ValueIdentity) -> super::schema::IdentityName {
    match i {
        ValueIdentity::FreshEveryRender => super::schema::IdentityName::FreshEveryRender,
        ValueIdentity::Unknown => super::schema::IdentityName::Unknown,
    }
}

/// The word `{anchor.identity}` renders.
fn identity_word(i: ValueIdentity) -> &'static str {
    match i {
        ValueIdentity::FreshEveryRender => "fresh-every-render",
        ValueIdentity::Unknown => "unknown",
    }
}

/// The word `{w.phase}` renders.
fn phase_word(p: WriterPhase) -> &'static str {
    match p {
        WriterPhase::Render => "render",
        WriterPhase::Effect => "effect",
        WriterPhase::Memo => "memo",
        WriterPhase::Callback => "callback",
        WriterPhase::Handler => "handler",
        WriterPhase::Deferred => "deferred",
        WriterPhase::Cleanup => "cleanup",
        WriterPhase::Unknown => "unknown",
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

/// Total projection `ReturnsVerdict` → verdict name (⊤ stays visible).
pub(crate) fn returns_name(v: ReturnsVerdict) -> ReturnsName {
    match v {
        ReturnsVerdict::Stable => ReturnsName::Stable,
        ReturnsVerdict::FreshReference => ReturnsName::FreshReference,
        ReturnsVerdict::Unknown => ReturnsName::Unknown,
    }
}

/// Unlike `stability`'s `per-render`, `fresh-reference` IS an allocation
/// claim: the identity defeats `Object.is` on every call.
pub(crate) fn returns_word(n: ReturnsName) -> &'static str {
    match n {
        ReturnsName::Stable => "a stable reference",
        ReturnsName::FreshReference => "a fresh reference per call",
        ReturnsName::Unknown => "a value of unknown identity",
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
