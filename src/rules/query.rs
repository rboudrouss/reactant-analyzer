//! Typed query surface (ADR-021): severity-by-construction.
//!
//! The must/may/⊤ distinction is encoded in types the compiler checks, so
//! violating it is a build error — even for a first-party Rust rule.
//!
//! - [`Certified`] is the proof token. Its constructor is **private to this
//!   module**, so only the query primitives here can mint one. [`super::Diagnostic::error`]
//!   is the sole `Error` constructor and it *requires* a `Certified`, so a
//!   [`May`] value has no path to an `Error` (type error).
//! - [`MustResult`] carries the token on its `All` arm; `Some`/`None` are MAY.
//! - [`StabilityVerdict`] is a **total** classifier: `Unknown` (⊤) is a returned
//!   variant, folded to the may side, and so cannot be dropped like a missing
//!   `match` arm.
//!
//! The single trusted core is the polarity *annotation* of each primitive (a
//! `may` mislabelled `must` would reopen Error-on-may) — now localised to the
//! definitions in this file, not spread across every rule.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::{
    domains::{Stability, StateValue, stores::Heap},
    engine::{AnalysisResult, DominatorTree, HookKind, ProgramAnalysisResult, compute_dominators},
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use super::{ConvergedEval, Note, SetterCall, Step, arg_is_call_free, local_bindings};

/// Where a certified fact lives, and the witness chain that proves it (ADR-019).
/// [`super::Diagnostic::error`] absorbs these into the finding, so the evidence's
/// own span/label/provenance ride the `Error` instead of being threaded by hand.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Provenance {
    pub range: Option<SourceRange>,
    pub hook_label: Option<HookLabel>,
    pub notes: Vec<Note>,
}

impl Provenance {
    pub fn at(range: Option<SourceRange>, hook_label: Option<HookLabel>) -> Self {
        Provenance {
            range,
            hook_label,
            notes: vec![],
        }
    }

    pub fn with_notes(mut self, notes: Vec<Note>) -> Self {
        self.notes = notes;
        self
    }
}

/// A certified MUST fact: the evidence plus its provenance.
///
/// The constructor ([`Certified::mint`]) is private to this module. Rule code in
/// sibling modules can only [`Certified::evidence`]/[`Certified::provenance`] —
/// it can never forge a token. This is the enforcement: an `Error` is reachable
/// only from a `Certified`, and a `Certified` only from a must-primitive here.
#[derive(Debug, Clone, PartialEq)]
pub struct Certified<E> {
    evidence: E,
    provenance: Provenance,
}

impl<E> Certified<E> {
    /// Mint a token. **Private to the query module** — the whole point.
    fn mint(evidence: E, provenance: Provenance) -> Self {
        Certified {
            evidence,
            provenance,
        }
    }

    pub fn evidence(&self) -> &E {
        &self.evidence
    }

    /// Discard the proof and keep the evidence — a *downgrade* (the safe
    /// direction: the fact loses its Error eligibility, never gains one).
    /// The demotion path of [`must_frozen_seed`]'s gate check.
    pub fn into_evidence(self) -> E {
        self.evidence
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// A three-valued MUST verdict. `All` carries the certified token (the only way
/// to obtain a `Certified` for a single-verdict primitive); `Some`/`None` are
/// MAY facts with no path to an `Error`.
///
/// (Deviates from the ADR-021 §1 literal `All(T)`: the token lives *inside* `All`
/// so `must_*` and the `Vec<Certified<_>>` primitives share one minting story.)
#[derive(Debug, Clone, PartialEq)]
pub enum MustResult<T> {
    /// Proven on **all** paths — carries the minted proof.
    All(Certified<T>),
    /// Proven on **some** but not all paths — a MAY fact (raw payload).
    Some(T),
    /// No qualifying evidence at all.
    None,
}

/// A MAY fact. There is no path from `May<_>` to an `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct May<T>(pub T);

impl<T> May<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

/// Total stability classifier for a rule-facing value probe.
///
/// Every [`Stability`] maps to exactly one variant (see [`StabilityVerdict::of`]);
/// `Unknown` (⊤) is a returned variant folded to the **may** side, so it cannot
/// be silently dropped. `Versioned` carries the change-driving slots (an empty
/// set means threshold-widened `VersionedTop`: "changes at some unknown slot").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StabilityVerdict {
    /// Same reference every render (the only Error-safe "stable" answer).
    Stable,
    /// Changes only at the named setter events (may bound). Empty = widened.
    Versioned(BTreeSet<(Symbol, HookLabel)>),
    /// A fresh reference every render (must bound).
    PerRender,
    /// ⊤ — no bound in either direction. Folded to the may side.
    Unknown,
}

impl StabilityVerdict {
    /// Total projection of the six-variant [`Stability`] lattice onto the
    /// rule-facing verdict. `Bottom` (⊥, unreachable/uninitialised) is **not**
    /// provably stable, so it folds to `Unknown` (may side) — matching the
    /// existing `is_stable` semantics (true only for `Stable`).
    pub fn of(stability: Stability) -> Self {
        match stability {
            Stability::Stable => StabilityVerdict::Stable,
            Stability::Versioned(slots) => StabilityVerdict::Versioned(slots),
            Stability::VersionedTop => StabilityVerdict::Versioned(BTreeSet::new()),
            Stability::PerRender => StabilityVerdict::PerRender,
            Stability::Bottom | Stability::Unknown => StabilityVerdict::Unknown,
        }
    }

    /// `true` iff the value is provably `Stable`. The sound gate: everything
    /// else (⊤, Versioned, PerRender) *may* change.
    pub fn is_stable(&self) -> bool {
        matches!(self, StabilityVerdict::Stable)
    }
}

/// Total stability classifier for an already-evaluated abstract value.
///
/// The value-level core of [`RuleCtx::stability_verdict`]; also the engine of
/// the FN fix (ADR-021 §5), where deps are evaluated once by the caller.
pub fn stability_verdict_of(val: &StateValue) -> StabilityVerdict {
    StabilityVerdict::of(val.to_stability())
}

/// The sole ⊤-safe stability-reachability probe (ADR-021 §3): `true` unless the
/// value is provably `Stable` (`⊤`/`Versioned`/`PerRender` → `true`). Replaces
/// the withdrawn `StateValue::is_unstable`, whose `PerRender`-only test let a
/// ⊤/`Versioned` value read as "not changing" — the shipped false negative.
pub fn may_change_of(val: &StateValue) -> May<bool> {
    May(!stability_verdict_of(val).is_stable())
}

/// Config carried on the ctx. A stub for now — the schema/format is deferred to
/// the frontend ADR (ADR-021 §4; config only matters for parameterized/external
/// rules, which do not exist yet).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuleConfig;

/// The single object a rule's `check`/`safe_check` binds to: the program result,
/// the component under analysis, its resolved [`AnalysisResult`], and the typed
/// query primitives (methods, added in the primitives section). The stable anchor
/// the future external frontends bind to (ADR-021 §4).
pub struct RuleCtx<'a> {
    program: &'a ProgramAnalysisResult,
    component: &'a Symbol,
    comp: &'a AnalysisResult<StateValue>,
    config: RuleConfig,
}

impl<'a> RuleCtx<'a> {
    /// Resolve `component`'s per-component result once. Panics if the component
    /// is absent (the dispatcher only builds a ctx for analysed components, and
    /// every rule indexed `result.components[component]` before).
    pub fn new(program: &'a ProgramAnalysisResult, component: &'a Symbol) -> Self {
        let comp = &program.components[component];
        RuleCtx {
            program,
            component,
            comp,
            config: RuleConfig,
        }
    }

    pub fn program(&self) -> &'a ProgramAnalysisResult {
        self.program
    }

    pub fn component(&self) -> &'a Symbol {
        self.component
    }

    /// The per-component analysis result — what every rule used to obtain via
    /// `&result.components[component]`.
    pub fn comp(&self) -> &'a AnalysisResult<StateValue> {
        self.comp
    }

    pub fn config(&self) -> &RuleConfig {
        &self.config
    }
}

// ── Query primitives (ADR-021 §3) ────────────────────────────────────────────
//
// Contract (normative): every primitive returns a polarity-typed verdict; ⊤
// folds to may inside; only must-primitives mint `Certified`. Adding one means
// following the contract — no new ADR.

/// Evidence that a set of blocks lies on every entry→exit path of a CFG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnAllPaths {
    pub blocks: BTreeSet<BlockId>,
}

/// Evidence that a block dominates every render exit (executes unconditionally).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DominatesAllExits {
    pub block: BlockId,
}

/// Evidence that a hook is called conditionally (its block does not dominate
/// every render exit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionalHookCall {
    pub label: HookLabel,
    pub span: Option<SourceRange>,
}

impl<'a> RuleCtx<'a> {
    fn eval_exit(&self, expr: &Expr) -> StateValue {
        let exit_env = self.comp.exit_env();
        self.comp.eval_in(&exit_env, expr, &mut Heap::new())
    }

    /// Total stability classifier for `expr` in the render-exit env (ADR-021 §3).
    pub fn stability_verdict(&self, expr: &Expr) -> StabilityVerdict {
        stability_verdict_of(&self.eval_exit(expr))
    }

    /// The sole ⊤-safe change probe (ADR-021 §3): `true` unless `expr` is
    /// provably `Stable`. Withdraws the FN-prone `is_unstable` from the surface.
    pub fn may_change(&self, expr: &Expr) -> May<bool> {
        may_change_of(&self.eval_exit(expr))
    }

    /// Every conditionally-called hook in the component: a hook whose block does
    /// not dominate every render exit. Packages the whole dominance ∀-exits
    /// check so a rule cannot under-quantify (ADR-021 §3).
    pub fn hook_is_conditional(&self) -> Vec<Certified<ConditionalHookCall>> {
        let cfg = &self.comp.render_cfg;
        let exits: Vec<BlockId> = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .collect();
        let domtree = DominatorTree::new(cfg);
        cfg_hook_calls(self.comp)
            .filter(|call| !matches!(call.kind, HookKind::Handler))
            .filter(|call| {
                exits
                    .iter()
                    .any(|&exit| !domtree.dominates(call.block_id, exit))
            })
            .map(|call| {
                let mut notes = Vec::new();
                if let Some((_, guard_span)) = guard_site(cfg, call.block_id) {
                    let step = Step::Branch {
                        desc: "a condition evaluated here — some render paths skip the hook"
                            .to_string(),
                    };
                    notes.push(Note {
                        message: step.render(&super::witness::fallback_name),
                        step,
                        hook_label: None,
                        range: guard_span,
                    });
                }
                Certified::mint(
                    ConditionalHookCall {
                        label: call.label,
                        span: call.span,
                    },
                    Provenance {
                        range: call.span,
                        hook_label: Some(call.label),
                        notes,
                    },
                )
            })
            .collect()
    }
}

fn cfg_hook_calls(
    comp: &AnalysisResult<StateValue>,
) -> impl Iterator<Item = &crate::engine::HookCallInfo> {
    comp.hook_calls.iter()
}

/// The unconditional-setter must-forward, promoted from the private
/// `derived_state::find_uncond_setter_call` (ADR-021 §3).
///
/// `All` iff `cfg` calls exactly one setter, with call-free args, on **every**
/// entry→exit path; `Some` when such a call exists but not on all paths (a MAY);
/// `None` when there is no single clean setter call at all. `restrict_to` scopes
/// the call-site scan to a block subset (`None` = whole CFG).
pub fn must_setter_on_all_paths(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    restrict_to: Option<&HashSet<BlockId>>,
) -> MustResult<SetterCall> {
    let mut call_sites: Vec<(BlockId, Var, Expr, Option<SourceRange>)> = vec![];
    for (bid, block) in &cfg.blocks {
        if restrict_to.is_some_and(|set| !set.contains(bid)) {
            continue;
        }
        for stmt in &block.stmts {
            if let Stmt::ExprStmt(expr, span) = stmt
                && let Some((var, arg)) = try_extract_setter_call(expr, setter_vars)
            {
                call_sites.push((*bid, var, arg.clone(), *span));
            }
        }
        if let Terminator::Return(expr) = &block.term
            && let Some((var, arg)) = try_extract_setter_call(expr, setter_vars)
        {
            call_sites.push((*bid, var, arg.clone(), None));
        }
    }
    if call_sites.is_empty() {
        return MustResult::None;
    }
    let target = call_sites[0].1.clone();
    let target_span = call_sites[0].3;
    let target_block = call_sites[0].0;
    // All sites must target the same setter, with call-free args, else no clean fact.
    if !call_sites.iter().all(|(_, v, _, _)| v == &target) {
        return MustResult::None;
    }
    let bindings = local_bindings(cfg);
    if !call_sites
        .iter()
        .all(|(_, _, arg, _)| arg_is_call_free(arg, &bindings, &mut HashSet::new()))
    {
        return MustResult::None;
    }
    let evidence = SetterCall {
        var: target,
        span: target_span,
        block_id: Some(target_block),
    };

    // must_in[B] = ∧ must_out[preds]; must_out[B] = must_in[B] ∨ called_in[B].
    let called_in: HashMap<BlockId, bool> = cfg
        .blocks
        .keys()
        .map(|&bid| (bid, call_sites.iter().any(|(b, _, _, _)| b == &bid)))
        .collect();
    let mut must_out: HashMap<BlockId, bool> = cfg.blocks.keys().map(|&bid| (bid, true)).collect();
    match must_out.get_mut(&cfg.entry) {
        Some(e) => *e = called_in[&cfg.entry],
        None => return MustResult::None,
    }
    let mut changed = true;
    while changed {
        changed = false;
        for &bid in cfg.blocks.keys() {
            if bid == cfg.entry {
                continue;
            }
            let preds = cfg.predecessors(bid);
            if preds.is_empty() {
                continue;
            }
            let must_in = preds.iter().all(|&p| *must_out.get(&p).unwrap_or(&false));
            let new_val = must_in || called_in[&bid];
            if must_out[&bid] != new_val {
                must_out.insert(bid, new_val);
                changed = true;
            }
        }
    }
    let exit_blocks: Vec<_> = cfg
        .blocks
        .values()
        .filter(|b| matches!(b.term, Terminator::Return(_) | Terminator::Unreachable))
        .collect();
    if exit_blocks.is_empty() {
        return MustResult::None;
    }
    if exit_blocks
        .iter()
        .all(|b| *must_out.get(&b.id).unwrap_or(&false))
    {
        MustResult::All(Certified::mint(evidence, Provenance::at(target_span, None)))
    } else {
        MustResult::Some(evidence)
    }
}

/// `All` iff every entry→exit path of `cfg` passes through one of `blocks`
/// (promotes `churn::on_all_paths`); `None` otherwise. The shared "on all paths"
/// must-forward (ADR-021 §3).
pub fn must_on_all_paths(cfg: &CFG, blocks: &HashSet<BlockId>) -> MustResult<OnAllPaths> {
    if super::churn::on_all_paths(cfg, blocks) {
        MustResult::All(Certified::mint(
            OnAllPaths {
                blocks: blocks.iter().copied().collect(),
            },
            Provenance::default(),
        ))
    } else {
        MustResult::None
    }
}

/// The dominance-over-exits relation for one render CFG, built once and queried
/// per block (the codebase's "build the `DominatorTree` once, don't recompute
/// per call × exit" discipline, lifted into the typed surface). The shared
/// dominance ∀-exits must-fact (ADR-021 §3); [`RuleCtx::hook_is_conditional`]
/// is its ∀-negation over hook calls.
pub struct ExitDominance {
    domtree: DominatorTree,
    exits: Vec<BlockId>,
}

impl ExitDominance {
    pub fn of(cfg: &CFG) -> Self {
        let exits = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .collect();
        ExitDominance {
            domtree: DominatorTree::new(cfg),
            exits,
        }
    }

    /// `All` iff `block` dominates every render exit (executes unconditionally);
    /// `None` otherwise.
    pub fn certify(&self, block: BlockId) -> MustResult<DominatesAllExits> {
        if self.exits.is_empty() {
            return MustResult::None;
        }
        if self
            .exits
            .iter()
            .all(|&exit| self.domtree.dominates(block, exit))
        {
            MustResult::All(Certified::mint(
                DominatesAllExits { block },
                Provenance::default(),
            ))
        } else {
            MustResult::None
        }
    }
}

/// One-shot [`ExitDominance`]: builds the relation and queries a single block.
/// Prefer [`ExitDominance::of`] when querying many blocks of the same CFG.
pub fn must_dominates_all_exits(cfg: &CFG, block: BlockId) -> MustResult<DominatesAllExits> {
    ExitDominance::of(cfg).certify(block)
}

/// Evidence that a `useState` initializer invokes a state setter every render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitSetterCall;

/// `All` iff `init` syntactically calls a state setter (a `StateSetter` or a
/// setter-var callee): the write runs on every render — a certain misuse
/// (ADR-021 §3). `None` otherwise. Mirrors the `Setter` branch of
/// `lazy_init::classify_init_effect` via the shared [`super::lazy_init::collect_callees`].
pub fn must_init_calls_setter(init: &Expr, setters: &HashSet<Var>) -> MustResult<InitSetterCall> {
    let mut callees = Vec::new();
    super::lazy_init::collect_callees(init, &mut callees);
    let calls_setter = callees.iter().any(|c| {
        let c = match c {
            Expr::TSAnnotated(inner) => inner.as_ref(),
            other => other,
        };
        matches!(c, Expr::StateSetter(_)) || matches!(c, Expr::Var(v) if setters.contains(v))
    });
    if calls_setter {
        MustResult::All(Certified::mint(InitSetterCall, Provenance::default()))
    } else {
        MustResult::None
    }
}

/// A state slot in an analyzed owner that provably moves: its setter is
/// referenced in the owner, with the first provable write site when one is
/// syntactically visible. The evidence carried by [`Motion::Proven`].
#[derive(Debug, Clone, PartialEq)]
pub struct MovingFeeder {
    pub slot: HookLabel,
    /// Owner's source-level name for the slot, pre-qualified for display
    /// ("state `text` of `Parent`").
    pub display: String,
    pub write_span: Option<SourceRange>,
}

/// What the domain proves about a seeding prop's motion across renders.
///
/// `Proven` carries its [`Certified`] token, minted HERE at the point of
/// knowledge ([`slot_write_evidence`] — the owner's setter is really
/// referenced), not vouched for by a caller-supplied boolean.
pub enum Motion {
    /// Provably never changes — kill.
    Still,
    /// Fed by a state slot that may actually be written in its owner.
    Proven(Certified<MovingFeeder>),
    /// May change, unproven (⊤ props, per-render values, unverifiable owner).
    Unproven,
}

/// Can `slot` ever be written in its owning component, and if so, where is
/// the first provable write site? `(false, _)` is a proof of stillness;
/// `(true, None)` means "referenced somewhere" without a direct call site
/// (setter passed onward).
fn slot_write_evidence(
    owner: &AnalysisResult<StateValue>,
    slot: HookLabel,
) -> (bool, Option<SourceRange>) {
    let setter_labels = super::all_setter_labels(owner);
    let may =
        super::stale_closure::may_written_slots(&owner.render_cfg, &owner.hooks, &setter_labels);
    if !may.contains(&slot) {
        return (false, None);
    }
    let setters: HashSet<Var> = setter_labels
        .iter()
        .filter(|(_, l)| **l == slot)
        .map(|(v, _)| v.clone())
        .collect();
    let render_fns = super::collect_fn_bindings(&owner.render_cfg);
    let mut spans: Vec<SourceRange> = std::iter::once(&owner.render_cfg)
        .chain(owner.hooks.iter().filter_map(|h| h.body_cfg()))
        .flat_map(|cfg| super::collect_setter_calls_with_extra(cfg, &setters, 2, &render_fns))
        .filter_map(|c| c.span)
        .collect();
    spans.sort_by_key(|r| r.pos_key());
    (true, spans.first().copied())
}

/// Classify the motion of a seeding prop from its abstract value (ADR-021 §3).
/// The `Proven` verdict mints its own [`Certified<MovingFeeder>`] — the only
/// way to obtain one — so downstream Error tiers consume a real proof.
pub fn classify_motion(val: &StateValue, result: &ProgramAnalysisResult) -> Motion {
    // Version labels live on the reference slot only (`to_stability` erases
    // them when another kind slot is ⊤) — check it first, like
    // `recompute_memo` does.
    if let Stability::Versioned(labels) = &val.reference {
        let mut unverifiable = false;
        for (owner, slot) in labels {
            let Some(owner_result) = result.components.get(owner) else {
                unverifiable = true;
                continue;
            };
            let (writable, write_span) = slot_write_evidence(owner_result, *slot);
            if writable {
                let owner_states = super::state_val_labels(&owner_result.render_cfg);
                let display = format!(
                    "state {} of `{owner}`",
                    super::state_slot_name(*slot, &owner_states)
                );
                return Motion::Proven(Certified::mint(
                    MovingFeeder {
                        slot: *slot,
                        display,
                        write_span,
                    },
                    Provenance::default(),
                ));
            }
        }
        return if unverifiable {
            Motion::Unproven
        } else {
            // Every feeding slot is owned by an analyzed component and its
            // setter is never referenced there: the prop provably never
            // changes (React state moves only through its setter).
            Motion::Still
        };
    }
    if val.reference == Stability::VersionedTop {
        return Motion::Unproven;
    }
    match val.to_stability() {
        Stability::Bottom | Stability::Stable => Motion::Still,
        _ => Motion::Unproven,
    }
}

/// `All` iff a proven moving feeder survives the idiomatic downgrades: the
/// setter does not escape, the seeds are not all seed-named (`initial*`/
/// `default*`), and the slot is locally written — the certain
/// `frozen-initial-state` Error (ADR-021 §3). Any failed gate *demotes* the
/// proof ([`Certified::into_evidence`]) — the fact survives as a MAY, its
/// Error eligibility does not. Forging is impossible: the input proof only
/// comes from [`classify_motion`].
pub fn must_frozen_seed(
    feeder: Certified<MovingFeeder>,
    escaped: bool,
    all_seed_named: bool,
    locally_written: bool,
) -> MustResult<MovingFeeder> {
    if !escaped && !all_seed_named && locally_written {
        MustResult::All(feeder)
    } else {
        MustResult::Some(feeder.into_evidence())
    }
}

/// Evidence that an effect churn cycle re-runs on every render on all paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectCycleProof;

/// `All` iff every edge of `cycle` is a proven must-rerun and the cycle stays
/// within one component — the certain `infinite-loop` Error (ADR-018 +
/// ADR-021 §3); `None` otherwise (cross-component or any may-edge → a MAY).
///
/// Both facts are re-derived HERE from the raw edges (strength per edge, slot
/// owners + effect carriers per component), the point of knowledge — not
/// trusted from caller-supplied booleans.
pub(super) fn must_effect_cycle(
    edges: &[super::churn_graph::ChurnEdge],
    cycle: &super::churn_graph::ChurnCycle,
) -> MustResult<EffectCycleProof> {
    use super::churn_graph::EdgeStrength;
    let all_must = cycle
        .edge_idx
        .iter()
        .all(|&i| edges[i].strength == EdgeStrength::Must);
    let mut comps: HashSet<&Symbol> = HashSet::new();
    for &i in &cycle.edge_idx {
        comps.insert(&edges[i].from.0);
        comps.insert(&edges[i].to.0);
        comps.insert(&edges[i].component);
    }
    if all_must && comps.len() == 1 {
        MustResult::All(Certified::mint(EffectCycleProof, Provenance::default()))
    } else {
        MustResult::None
    }
}

/// Evidence that a state object is mutated in place while its setter is called
/// with the same reference in the same trigger — React's `Object.is` sees no
/// change and skips the re-render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SameRefMutation;

/// `All` iff a mutation site and a setter-call site share a container (the same
/// event/trigger scope) — the certain `state-mutation` Error (ADR-021 §3);
/// `None` otherwise (a MAY: mutation and set on different triggers).
pub fn must_same_ref_mutation(
    mutation_containers: &HashSet<usize>,
    setter_containers: &HashSet<usize>,
) -> MustResult<SameRefMutation> {
    if mutation_containers
        .intersection(setter_containers)
        .next()
        .is_some()
    {
        MustResult::All(Certified::mint(SameRefMutation, Provenance::default()))
    } else {
        MustResult::None
    }
}

fn try_extract_setter_call<'e>(
    expr: &'e Expr,
    setter_vars: &HashSet<Var>,
) -> Option<(Var, &'e Expr)> {
    if let Expr::Call { fn_, args } = expr
        && let Expr::Var(name) = fn_.as_ref()
        && setter_vars.contains(name)
    {
        Some((name.clone(), args.first()?))
    } else {
        None
    }
}

/// `true` iff `to` is reachable from `from` by following CFG edges.
fn reaches(cfg: &CFG, from: BlockId, to: BlockId) -> bool {
    let mut seen = HashSet::new();
    let mut stack = vec![from];
    while let Some(b) = stack.pop() {
        if b == to {
            return true;
        }
        if seen.insert(b) {
            stack.extend(cfg.successors(b));
        }
    }
    false
}

/// Site of the closest dominating `Branch` that actually makes `block`
/// conditional (a successor that never reaches `block`). Moved in from the
/// former `conditional_hook::guard_site` — the guard-blaming witness for
/// [`RuleCtx::hook_is_conditional`].
fn guard_site(cfg: &CFG, block: BlockId) -> Option<(BlockId, Option<SourceRange>)> {
    let doms = compute_dominators(cfg);
    let dominators = doms.get(&block)?;
    dominators
        .iter()
        .filter(|&&d| {
            d != block
                && matches!(
                    cfg.blocks.get(&d).map(|b| &b.term),
                    Some(Terminator::Branch { .. })
                )
                && cfg.successors(d).iter().any(|&s| !reaches(cfg, s, block))
        })
        .max_by_key(|&&d| doms.get(&d).map_or(0, |s| s.len()))
        .map(|&d| {
            let guard = cfg.blocks.get(&d);
            let span = guard
                .and_then(|b| match &b.term {
                    Terminator::Branch { span, .. } => *span,
                    _ => None,
                })
                .or_else(|| {
                    guard.and_then(|b| b.stmts.last()).and_then(|s| match s {
                        Stmt::Let { span, .. } => *span,
                        Stmt::ExprStmt(_, span) => *span,
                        Stmt::Assign { span, .. } => *span,
                        Stmt::MemberWrite { span, .. } => *span,
                    })
                });
            (d, span)
        })
}
