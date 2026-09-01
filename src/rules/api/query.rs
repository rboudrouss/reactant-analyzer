//! Typed query surface (ADR-021): severity-by-construction.
//!
//! The must/may/⊤ distinction is encoded in types the compiler checks, so
//! violating it is a build error — even for a first-party Rust rule.
//!
//! - [`Certified`] is the proof token. Its constructor is **private to this
//!   module**, so only the query primitives here can mint one. [`crate::rules::Diagnostic::error`]
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
        expr::{Expr, Prim},
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use crate::rules::api::cache::ProgramCache;
use crate::rules::helpers::mount::MountCoupling;
use crate::rules::{ConvergedEval, Note, SetterCall, Step, arg_is_call_free, local_bindings};

/// Where a certified fact lives, and the witness chain that proves it (ADR-019).
///
/// [`crate::rules::Diagnostic::error`] absorbs these into the finding, so a
/// primitive that knows where its proof sits does not have to hand the position
/// back to the rule through a side channel. It is a *default*, not a lock: a
/// rule that has a better anchor still refines it with `with_range`/`with_label`
/// (`state-mutation` points at the mutation site, not at the write that proves
/// the cycle).
///
/// Some primitives legitimately leave it [`Default`]: they are handed the shape
/// of the proof and not its location — `must_same_ref_mutation` receives
/// container ids, `must_init_calls_setter` an `Expr` (which carries no span),
/// `must_on_all_paths` a block set. Empty provenance there is honest, not an
/// omission; enriching it would mean widening those signatures for a position
/// the rule already has.
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
/// The constructor (`Certified::mint`) is private to this module. Rule code in
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
    /// Changes on every render (must bound) — **kind-agnostic motion**, not
    /// necessarily a fresh allocation (ADR-017): a numeric slot converged to a
    /// non-point interval lands here alongside an object literal. A consumer
    /// that needs "defeats `Object.is` because the identity is new" wants
    /// [`crate::domains::StateValue::is_unstable_reference_only`] instead.
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

/// ⊤-total classifier of what a function-valued call-site argument *returns*
/// (ADR-023 §3). Asks the identity question, not the stability one: a store
/// selector crashes zustand v5 when its return defeats `Object.is` — a fresh
/// reference per call — while a moving *primitive* is value-compared and safe,
/// which is why `stability`'s `per-render` (kind-agnostic motion) is the wrong
/// vocabulary here and the `args` edge does not admit it.
///
/// A classifier, not a must-primitive (ADR-023 Limitations): it mints nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnsVerdict {
    /// Provably the same reference on every call.
    Stable,
    /// A reference whose identity is fresh on every call — defeats `Object.is`.
    FreshReference,
    /// Everything else — primitives, mixed kinds, ⊤, or an argument the
    /// engine could not resolve (Var-bound, imported). The may side.
    Unknown,
}

/// Project an abstract return value onto [`ReturnsVerdict`] — the same
/// three-way coarsening `SummaryValue` established for library-hook returns.
pub fn returns_verdict_of(val: &StateValue) -> ReturnsVerdict {
    use crate::domains::impls::Stability;
    if *val == StateValue::reference(Stability::Stable) {
        ReturnsVerdict::Stable
    } else if val.is_unstable_reference_only() {
        ReturnsVerdict::FreshReference
    } else {
        ReturnsVerdict::Unknown
    }
}

/// The sole ⊤-safe stability-reachability probe (ADR-021 §3): `true` unless the
/// value is provably `Stable` (`⊤`/`Versioned`/`PerRender` → `true`). Replaces
/// the withdrawn `StateValue::is_unstable`, whose `PerRender`-only test let a
/// ⊤/`Versioned` value read as "not changing" — the shipped false negative.
pub fn may_change_of(val: &StateValue) -> May<bool> {
    May(!stability_verdict_of(val).is_stable())
}

/// Per-rule options store (ADR-022 §4). Values are leaf constants, validated
/// against the rule's declared params by the loader — no native rule declares
/// params in v1, so the mechanism exists and waits for a client (Tier-A rules
/// consume their options at load time, baked into the resolved rule).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleConfig {
    options: serde_json::Map<String, serde_json::Value>,
}

impl RuleConfig {
    pub fn new(options: serde_json::Map<String, serde_json::Value>) -> Self {
        RuleConfig { options }
    }

    pub fn option(&self, key: &str) -> Option<&serde_json::Value> {
        self.options.get(key)
    }
}

/// The single object a rule's `check`/`safe_check` binds to: the program result,
/// the component under analysis, its resolved [`AnalysisResult`], and the typed
/// query primitives (methods, added in the primitives section). The stable anchor
/// the future external frontends bind to (ADR-021 §4).
pub struct RuleCtx<'a> {
    cache: CacheRef<'a>,
    component: &'a Symbol,
    comp: &'a AnalysisResult<StateValue>,
    config: RuleConfig,
}

/// The ctx's [`ProgramCache`]: shared with every other component of the same
/// program when the dispatcher supplies one, private when a caller builds a
/// one-off ctx (single-component callers, tests).
enum CacheRef<'a> {
    Shared(&'a ProgramCache<'a>),
    Own(ProgramCache<'a>),
}

impl<'a> CacheRef<'a> {
    fn get(&self) -> &ProgramCache<'a> {
        match self {
            CacheRef::Shared(c) => c,
            CacheRef::Own(c) => c,
        }
    }
}

impl<'a> RuleCtx<'a> {
    /// Resolve `component`'s per-component result once. Panics if the component
    /// is absent (the dispatcher only builds a ctx for analysed components, and
    /// every rule indexed `result.components[component]` before).
    ///
    /// The ctx gets a private [`ProgramCache`], so whole-program data is
    /// recomputed for it — the dispatcher uses [`RuleCtx::cached`] to share one
    /// cache across the whole program instead.
    pub fn new(program: &'a ProgramAnalysisResult, component: &'a Symbol) -> Self {
        Self::with_config(program, component, RuleConfig::default())
    }

    /// Like [`RuleCtx::new`], carrying per-rule options (ADR-022 §4).
    pub fn with_config(
        program: &'a ProgramAnalysisResult,
        component: &'a Symbol,
        config: RuleConfig,
    ) -> Self {
        Self::build(CacheRef::Own(ProgramCache::new(program)), component, config)
    }

    /// The dispatcher's constructor: every component of a program binds to the
    /// same [`ProgramCache`], so program-level structures are built once
    /// instead of once per component (issue #86).
    pub fn cached(cache: &'a ProgramCache<'a>, component: &'a Symbol, config: RuleConfig) -> Self {
        Self::build(CacheRef::Shared(cache), component, config)
    }

    fn build(cache: CacheRef<'a>, component: &'a Symbol, config: RuleConfig) -> Self {
        let comp = &cache.get().program().components[component];
        RuleCtx {
            cache,
            component,
            comp,
            config,
        }
    }

    pub fn program(&self) -> &'a ProgramAnalysisResult {
        self.cache.get().program()
    }

    /// The program-scoped derived-data cache backing this ctx.
    pub(in crate::rules) fn cache(&self) -> &ProgramCache<'a> {
        self.cache.get()
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

    /// Returns-verdict of argument `arg` of the custom hook labelled `label`
    /// (ADR-023 §3). ⊤-total: an argument the engine did not resolve — not an
    /// inline `FnLit`, or no such row — answers `Unknown`. The evaluation
    /// happened during the fixpoint (`AnalysisResult::custom_arg_returns`);
    /// this reader only projects it.
    pub fn returns_verdict(&self, label: HookLabel, arg: usize) -> ReturnsVerdict {
        self.comp
            .custom_arg_returns
            .get(&(label, arg))
            .map_or(ReturnsVerdict::Unknown, returns_verdict_of)
    }

    /// Every conditionally-called hook in the component: a hook whose block does
    /// not dominate every render exit. Packages the whole dominance ∀-exits
    /// check so a rule cannot under-quantify (ADR-021 §3).
    pub fn hook_is_conditional(&self) -> Vec<Certified<ConditionalHookCall>> {
        let cfg = &self.comp.render_cfg;
        // One owner for "what the exits are": this used to keep its own copy of
        // the enumeration and its own `DominatorTree`, which could disagree with
        // `ExitDominance` about reachability.
        let dominance = ExitDominance::of(cfg);
        cfg_hook_calls(self.comp)
            .filter(|call| !matches!(call.kind, HookKind::Handler))
            .filter(|call| dominance.may_be_skipped(call.block_id))
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
    if crate::rules::helpers::churn::on_all_paths(cfg, blocks) {
        // No provenance: the proof is a property of the whole CFG, not of one
        // position in it. The blocks it holds are on the evidence.
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
        // Reachable exits only. A `Return` no path can arrive at is not an exit:
        // lowering seals a fall-through tail as `Return(undefined)`, and an
        // `if`/`else` whose both branches returned leaves that tail orphaned —
        // counting it would make every hook before the branch fail to dominate
        // "all exits" and report as conditional at the **Error** tier.
        let reachable = cfg.reachable_blocks();
        let exits = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .filter(|id| reachable.contains(id))
            .collect();
        ExitDominance {
            domtree: DominatorTree::new(cfg),
            exits,
        }
    }

    /// `true` when `block` may be skipped on some render path — the *rule-facing*
    /// negation of [`Self::certify`], and deliberately not its `MustResult::None`
    /// case: a CFG with no reachable exit proves nothing in either direction, so
    /// nothing is skippable there.
    pub fn may_be_skipped(&self, block: BlockId) -> bool {
        !self.exits.is_empty()
            && self
                .exits
                .iter()
                .any(|&exit| !self.domtree.dominates(block, exit))
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
            // No provenance: a dominance relation over exits has no single
            // position — the block it holds is on the evidence.
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
/// `lazy_init::classify_init_effect` via the shared `collect_callees`.
pub fn must_init_calls_setter(init: &Expr, setters: &HashSet<Var>) -> MustResult<InitSetterCall> {
    let mut callees = Vec::new();
    crate::rules::collect_callees(init, &mut callees);
    let calls_setter = callees.iter().any(|c| {
        let c = match c {
            Expr::TSAnnotated(inner) => inner.as_ref(),
            other => other,
        };
        matches!(c, Expr::StateSetter(_)) || matches!(c, Expr::Var(v) if setters.contains(v))
    });
    if calls_setter {
        // No provenance: an `Expr` carries no span, so the position of the
        // offending call is something the *caller* holds and this primitive
        // never sees.
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
    /// Component the slot belongs to — the half of the version label a rule
    /// needs to ask further questions about the slot (who writes it, when the
    /// consumer is mounted relative to those writes).
    pub owner: Symbol,
    pub slot: HookLabel,
    /// Owner's source-level name for the slot, pre-qualified for display
    /// ("state `text` of `Parent`").
    pub display: String,
    pub write_span: Option<SourceRange>,
}

/// What the domain proves about a seeding prop's motion across renders.
///
/// `Proven` carries its [`Certified`] token, minted HERE at the point of
/// knowledge (`slot_write_evidence` — the owner's setter is really
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
    let setter_labels = crate::rules::all_setter_labels(owner);
    let may = crate::rules::may_written_slots(&owner.render_cfg, &owner.hooks, &setter_labels);
    if !may.contains(&slot) {
        return (false, None);
    }
    let setters: HashSet<Var> = setter_labels
        .iter()
        .filter(|(_, l)| **l == slot)
        .map(|(v, _)| v.clone())
        .collect();
    let render_fns = crate::rules::collect_fn_bindings(&owner.render_cfg);
    let mut spans: Vec<SourceRange> = std::iter::once(&owner.render_cfg)
        .chain(owner.hooks.iter().filter_map(|h| h.body_cfg()))
        .flat_map(|cfg| {
            crate::rules::collect_setter_calls_with_extra(cfg, &setters, 2, &render_fns)
        })
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
                let owner_states = crate::rules::state_val_labels(&owner_result.render_cfg);
                let display = format!(
                    "state {} of `{owner}`",
                    crate::rules::state_slot_name(*slot, &owner_states)
                );
                return Motion::Proven(Certified::mint(
                    MovingFeeder {
                        owner: owner.clone(),
                        slot: *slot,
                        display,
                        write_span,
                    },
                    // The write that moves the feeding slot *is* the proof, and
                    // this is where it was found. The consuming rule anchors
                    // the finding at the seed site instead — a better place to
                    // point a user — but the token now says where the proof
                    // lives without anyone reading back into the evidence.
                    Provenance::at(write_span, None),
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
/// `default*`), the slot is locally written, and the feeder's move is
/// *observable* by a mounted consumer — the certain `frozen-initial-state`
/// Error (ADR-021 §3). Any failed gate *demotes* the proof
/// ([`Certified::into_evidence`]) — the fact survives as a MAY, its Error
/// eligibility does not. Forging is impossible: the input proof only comes
/// from [`classify_motion`].
///
/// `mount` is the call sites' verdict ([`MountCoupling`], issue #95): anything
/// but `Free` means a mounted consumer may never observe the move, so the
/// freeze is real but its Error certainty is gone — a MAY.
pub fn must_frozen_seed(
    feeder: Certified<MovingFeeder>,
    escaped: bool,
    all_seed_named: bool,
    locally_written: bool,
    mount: MountCoupling,
) -> MustResult<MovingFeeder> {
    if !escaped && !all_seed_named && locally_written && mount == MountCoupling::Free {
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
pub(in crate::rules) fn must_effect_cycle(
    edges: &[crate::rules::helpers::churn_graph::ChurnEdge],
    cycle: &crate::rules::helpers::churn_graph::ChurnCycle,
) -> MustResult<EffectCycleProof> {
    use crate::rules::helpers::churn_graph::EdgeStrength;
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
        // The cycle's first edge is where the proof starts: the write that
        // re-triggers the effect carrying it. Both are on the edge already, so
        // the token carries them rather than leaving the rule to re-derive
        // them from a `Vec` index.
        let first = cycle.edge_idx.first().map(|&i| &edges[i]);
        let provenance = match first {
            Some(e) => Provenance::at(e.write_span, Some(e.effect_label)),
            None => Provenance::default(),
        };
        MustResult::All(Certified::mint(EffectCycleProof, provenance))
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
        // No provenance: the inputs are container ids, so neither the mutation
        // site nor the setter call reaches this primitive. The rule anchors it.
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

// ── Effect cleanup ────────────────────────────────────────────────────────────

/// What an effect body returns, from the point of view of teardown.
///
/// Three-valued on purpose, and `Unknown` folds to the **may** side: it means
/// "there may be a cleanup", so it can never be read as an absence. Only
/// [`CleanupVerdict::Absent`] — every exit returns nothing at all — is a claim,
/// and it is the only one a rule may act on. The asymmetry is the point: an
/// effect that returns *something* is one whose author wrote a teardown, or
/// wrote something we cannot classify, and advice is wrong in both cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupVerdict {
    /// Some exit returns a function.
    Present,
    /// No exit returns anything: every `return` is bare and the body otherwise
    /// falls off the end.
    Absent,
    /// Something is returned that we cannot classify as a function or not —
    /// a call result, an unresolvable variable.
    Unknown,
}

/// Classify an effect body's teardown. `fn_lit_binding`'s certainty bar is
/// reused for `return unsubscribe`: a variable bound to exactly one function
/// literal counts as a cleanup, a re-bound or imported one is `Unknown`.
pub fn cleanup_verdict(body: &CFG) -> CleanupVerdict {
    let mut verdict = CleanupVerdict::Absent;
    for block in body.blocks.values() {
        let Terminator::Return(expr) = &block.term else {
            continue;
        };
        match classify_returned(expr, body) {
            // One cleanup on one path is a cleanup: the rule is about the
            // author forgetting teardown entirely, not about a path missing it.
            CleanupVerdict::Present => return CleanupVerdict::Present,
            CleanupVerdict::Unknown => verdict = CleanupVerdict::Unknown,
            CleanupVerdict::Absent => {}
        }
    }
    verdict
}

fn classify_returned(expr: &Expr, body: &CFG) -> CleanupVerdict {
    match expr.peel_ts() {
        Expr::FnLit { .. } => CleanupVerdict::Present,
        // `return;` and `return undefined;` — the author returned nothing.
        Expr::Lit(Prim::Unit) => CleanupVerdict::Absent,
        Expr::Var(v) => match crate::rules::fn_lit_binding(v, body) {
            Some(_) => CleanupVerdict::Present,
            None => CleanupVerdict::Unknown,
        },
        _ => CleanupVerdict::Unknown,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::cfg::{BasicBlock, Edge, EdgeKind, Terminator};
    use crate::ir::expr::Prim;

    fn set_call(line: u32) -> Stmt {
        Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setX".to_string())),
                args: vec![Expr::Lit(Prim::Int(0))],
            },
            Some(SourceRange {
                file: crate::ir::FileTable::default().intern(std::path::Path::new("t.tsx")),
                line,
                col: 0,
            }),
        )
    }

    /// When one setter is called from several blocks, the witness must name
    /// the call site of the *lowest* block — lowering order, i.e. source
    /// order. `CFG::blocks` is a `BTreeMap` so the pick is the same on every
    /// run; under the former `HashMap` it followed the per-process hash seed
    /// and the reported span flipped between runs of the same binary.
    #[test]
    fn setter_witness_names_the_first_call_site_in_block_order() {
        // 0 → 1 → 2, `setX` called in 1 and 2, inserted back to front.
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![set_call(20)],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![set_call(10)],
                term: Terminator::Jump(2),
            },
        );
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(1),
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 1,
                    to: 2,
                    kind: EdgeKind::Unconditional,
                },
            ],
        };

        let setters = HashSet::from(["setX".to_string()]);
        let MustResult::All(proof) = must_setter_on_all_paths(&cfg, &setters, None) else {
            panic!("the setter is called on every path — expected an all-paths proof");
        };
        assert_eq!(proof.evidence().block_id, Some(1));
        assert_eq!(proof.evidence().span.map(|s| s.line), Some(10));
    }
}
