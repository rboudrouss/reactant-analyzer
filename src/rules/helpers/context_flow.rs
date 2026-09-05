//! The `context_consumers` reachability relation (#115, ADR-032): a
//! `useContext` call site, the canonical context cell it reads, and whether any
//! component that may render it provides that cell.
//!
//! Built as a **post-pass over converged results**, not during the fixpoint. It
//! feeds nothing to the analysis — no context *value* is modelled here, and
//! #28's open post-pass-vs-unified-phases decision is neither answered nor
//! foreclosed by it.
//!
//! ## Why the ancestry gate is the whole design
//!
//! The verdict is an ABSENCE — "no provider on any path that reaches this
//! consumer" — and an absence is only as trustworthy as the paths you can see.
//! `analyze_program` runs two phases, and phase 2 records no call-graph edges,
//! so an unreached component is indistinguishable from a genuine root through
//! `callers_of` alone. Reading that as "no callers" would fire the rule on
//! every consumer whose real parent the analysis never entered.
//!
//! Two gates close it, and a row survives only if both pass:
//!
//! 1. **Complete ancestry** (#110): every component on the way up was
//!    inter-analysed. `complete_ancestry` answers `None` — unknown, not empty —
//!    the moment one was not.
//! 2. **The syntactic completion pass**: a phase-2 component that *mentions*
//!    anything in the closure may be a parent the call graph never recorded, so
//!    the row is dropped on any such mention. A `CompApp` scan is a sound
//!    over-approximation of "may render".

use std::collections::{HashMap, HashSet};

use crate::domains::StateValue;
use crate::engine::{AnalysisResult, ProgramAnalysisResult};
use crate::ir::{
    ComponentId, ContextId, ModuleConstInit, SourceRange,
    hooks::HookEntry,
    types::{HookLabel, Var},
};

use super::providers::collect_provider_sites;

/// Whether a provider of the consumer's context sits on a path that reaches it.
///
/// MAY-typed and positive-only. `NoneOnAnalyzedPaths` is what the analysed
/// paths showed, and the name says exactly that: the residue the weakened note
/// records — an unanalyzed mounting shell above a root, an inline-arrow
/// provider (#30), a value-position component reference (#63) — all land here
/// without being a proof of absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::rules) enum ProviderVerdict {
    /// Some component in the closure renders a provider of this cell.
    ProviderSeen,
    /// None did, on the paths the analysis could complete.
    NoneOnAnalyzedPaths,
}

/// One `useContext` call whose ancestry the analysis could complete.
///
/// The cell it reads is not a field: the pairing is decided during
/// [`ContextConsumers::build`] and the canonical id has no reader afterwards —
/// a message names the LOCAL binding, which is what the author wrote.
#[derive(Debug, Clone)]
pub(in crate::rules) struct ConsumerRow {
    /// The component holding the call.
    pub component: ComponentId,
    /// The local name the call names the context by — what a message shows.
    pub name: Var,
    pub label: HookLabel,
    pub span: Option<SourceRange>,
    pub verdict: ProviderVerdict,
}

/// The program's context-consumer relation, built once per program.
#[derive(Debug, Default)]
pub(in crate::rules) struct ContextConsumers {
    rows: Vec<ConsumerRow>,
}

impl ContextConsumers {
    /// The rows belonging to `component`, in call order.
    pub(in crate::rules) fn of(&self, component: ComponentId) -> Vec<&ConsumerRow> {
        self.rows
            .iter()
            .filter(|r| r.component == component)
            .collect()
    }

    pub(in crate::rules) fn build(prog: &ProgramAnalysisResult) -> Self {
        let providers = provider_index(prog);
        if providers.is_empty() && consumer_sites(prog).is_empty() {
            return Self::default();
        }
        // Component names a phase-2 body syntactically instantiates. Every one
        // of these may be a parent the call graph never recorded, because a
        // phase-2 component is analysed with no `InterCtx` and records no edges.
        let unreached_refs = unreached_component_refs(prog);

        let mut rows: Vec<ConsumerRow> = Vec::new();
        for (component, name, context, label, span) in consumer_sites(prog) {
            // Gate 1: unknown ancestry is not empty ancestry (#110).
            let Some(ancestors) = prog.complete_ancestry(component) else {
                continue;
            };
            // A cut recursion means the closure was never walked to the end.
            if prog.recursive_components.contains(&component)
                || ancestors
                    .iter()
                    .any(|a| prog.recursive_components.contains(a))
            {
                continue;
            }
            // Gate 2: an unreached component that mentions anything in the
            // closure may sit above it, provider and all.
            if unreached_refs.contains(&component)
                || ancestors.iter().any(|a| unreached_refs.contains(a))
            {
                continue;
            }
            // A component that renders the provider it also consumes reads the
            // OUTER value, so its own provider is not really a hit. Counting it
            // anyway only suppresses — and the alternative is firing on a shape
            // people write deliberately.
            let seen = std::iter::once(&component)
                .chain(ancestors.iter())
                .any(|c| providers.get(c).is_some_and(|ids| ids.contains(&context)));
            rows.push(ConsumerRow {
                component,
                name,
                label,
                span,
                verdict: if seen {
                    ProviderVerdict::ProviderSeen
                } else {
                    ProviderVerdict::NoneOnAnalyzedPaths
                },
            });
        }
        rows.sort_by(|a, b| (&a.component, a.label).cmp(&(&b.component, b.label)));
        ContextConsumers { rows }
    }
}

/// `component → the cells it provides`.
fn provider_index(prog: &ProgramAnalysisResult) -> HashMap<ComponentId, HashSet<ContextId>> {
    let mut out: HashMap<ComponentId, HashSet<ContextId>> = HashMap::new();
    for (id, comp) in &prog.components {
        let ids: HashSet<ContextId> = collect_provider_sites(comp)
            .into_iter()
            .map(|s| s.context_id.clone())
            .collect();
        if !ids.is_empty() {
            out.insert(*id, ids);
        }
    }
    out
}

/// Every `useContext(X)` whose `X` resolves to a proven context cell.
///
/// The gate is the *argument*, not the hook's name or import: a call named
/// `useContext` whose argument is not a proven `createContext` cell says
/// nothing this relation can pair, and one that is could only have come from
/// React's.
fn consumer_sites(
    prog: &ProgramAnalysisResult,
) -> Vec<(ComponentId, Var, ContextId, HookLabel, Option<SourceRange>)> {
    let mut out = Vec::new();
    for (comp_id, comp) in &prog.components {
        for (var, id, label, span) in context_reads(comp) {
            out.push((*comp_id, var, id, label, span));
        }
    }
    out.sort_by_key(|r| (r.0, r.3));
    out
}

fn context_reads(
    comp: &AnalysisResult<StateValue>,
) -> Vec<(Var, ContextId, HookLabel, Option<SourceRange>)> {
    comp.hooks
        .iter()
        .filter_map(|h| {
            let HookEntry::Custom {
                label,
                name,
                args,
                span,
                ..
            } = h
            else {
                return None;
            };
            if name != "useContext" {
                return None;
            }
            let crate::ir::expr::Expr::Var(v) = args.first()?.peel_ts() else {
                return None;
            };
            match comp.module_consts.get(v) {
                Some(ModuleConstInit::Context(id)) => Some((v.clone(), id.clone(), *label, *span)),
                _ => None,
            }
        })
        .collect()
}

/// The components a body phase 1 never reached may instantiate.
///
/// The caller reads a hit as "this may sit above the consumer" and drops the
/// row, so **over**-inclusion is the safe direction and a missed component is
/// the forbidden one. A resolved reference contributes the one component its
/// origin proves; an unresolved one contributes every component that could be
/// the one it wrote (#7).
fn unreached_component_refs(prog: &ProgramAnalysisResult) -> HashSet<ComponentId> {
    let mut refs = HashSet::new();
    for (id, comp) in &prog.components {
        if prog.was_inter_analyzed(*id) {
            continue;
        }
        crate::engine::root_detector::collect_compapp_refs(
            &comp.render_cfg,
            &comp.hooks,
            &mut refs,
        );
    }
    let table = &prog.component_table;
    let mut out: HashSet<ComponentId> = HashSet::new();
    for r in &refs {
        match &r.origin {
            Some(o) => out.extend(table.id_of(o)),
            None => out.extend(table.ids_named(&r.name)),
        }
    }
    out
}
