use std::collections::{HashMap, HashSet};

use crate::{
    domains::{impls::StateValue, stores::SharedStateStore},
    engine::analysis_result::AnalysisResult,
    ir::{
        ModuleTable,
        source_range::{FileTable, SourceRange},
        types::Symbol,
    },
};

pub type SymbolPair = (Symbol, Symbol);

/// Program-level analysis result spanning all components.
/// Rules receive `&ProgramAnalysisResult` and access per-component data via `components`.
#[derive(Debug)]
pub struct ProgramAnalysisResult {
    pub components: HashMap<Symbol, AnalysisResult<StateValue>>,
    pub shared_state: SharedStateStore,
    pub call_graph: ComponentCallGraph,
    /// Components whose recursion was cut off (received ⊤ result).
    pub recursive_components: HashSet<Symbol>,
    pub stats: AnalysisStats,
    /// Resolves the [`crate::ir::FileId`] carried by every [`SourceRange`]
    /// (ADR-019). Empty when the IR was built by hand (unit tests).
    pub file_table: FileTable,
    /// Lowered utility functions, exposed to witness producers so rules can
    /// resolve a callee name to its body (`witness::resolve_and_classify`,
    /// ADR-019). Empty for hand-built IR.
    pub function_registry: crate::engine::FunctionRegistry,
    /// Per-file directive prologue and import edges (ADR-026 §1). Empty when
    /// the IR was built by hand — a rule reading it must treat "absent" as
    /// *unproven*, never as a proven negative.
    pub module_table: ModuleTable,
    /// Components analysed in **phase 1** — the roots plus everything reached
    /// top-down from them with an `InterCtx` (#110).
    ///
    /// **The reading discipline, and why the field exists.** Phase 2 sweeps
    /// every component phase 1 did not reach and analyses it intra-only, with
    /// no `InterCtx`, so it records no call-graph edges. Through
    /// [`ComponentCallGraph::callers_of`] alone a phase-2 component is
    /// therefore indistinguishable from a genuine root: both answer "no
    /// callers". For a component NOT in this set, an empty `callers_of` means
    /// **unknown ancestry**, never "proven root".
    ///
    /// Any relation that walks ancestry to conclude something is *absent* —
    /// no provider above this consumer, no caller that could hold the setter —
    /// must consult this set first, or it fails open on exactly the components
    /// whose parents it cannot see. Empty for hand-built IR, which reads as
    /// "nothing was inter-analysed": the conservative answer.
    pub phase1_reached: HashSet<Symbol>,
}

impl ProgramAnalysisResult {
    /// Was `comp` analysed top-down in phase 1, with its callers' props and
    /// callbacks flowed in?
    ///
    /// `false` means one of two things the analysis cannot tell apart — the
    /// component is genuinely unreachable, or nothing that renders it was a
    /// root — so a consumer must treat its ancestry as unknown rather than
    /// empty (#110, #20).
    pub fn was_inter_analyzed(&self, comp: &Symbol) -> bool {
        self.phase1_reached.contains(comp)
    }

    /// The transitive caller closure of `comp`, or `None` when any component on
    /// the way up was not inter-analysed — the ancestry is then unknown, not
    /// empty, and a caller reasoning about absence must not proceed.
    ///
    /// Cycle-safe: a recursive component's closure includes itself and
    /// terminates. `comp` itself must be inter-analysed, otherwise even its
    /// direct callers are unknown.
    pub fn complete_ancestry(&self, comp: &Symbol) -> Option<HashSet<Symbol>> {
        if !self.was_inter_analyzed(comp) {
            return None;
        }
        let mut seen: HashSet<Symbol> = HashSet::new();
        let mut queue = vec![comp.clone()];
        while let Some(cur) = queue.pop() {
            for caller in self.call_graph.callers_of(&cur) {
                if !self.was_inter_analyzed(caller) {
                    return None;
                }
                if seen.insert(caller.clone()) {
                    queue.push(caller.clone());
                }
            }
        }
        Some(seen)
    }
}

/// Directed call graph: caller → list of call sites.
#[derive(Debug, Default, Clone)]
pub struct ComponentCallGraph {
    pub edges: HashMap<Symbol, Vec<CallSite>>,
}

impl ComponentCallGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_edge(&mut self, caller: Symbol, site: CallSite) {
        self.edges.entry(caller).or_default().push(site);
    }

    pub fn callees_of(&self, comp: &Symbol) -> &[CallSite] {
        self.edges.get(comp).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn callers_of(&self, comp: &Symbol) -> Vec<&Symbol> {
        self.edges
            .iter()
            .filter(|(_, sites)| sites.iter().any(|s| &s.callee == comp))
            .map(|(caller, _)| caller)
            .collect()
    }
}

/// One instantiation of a child component inside a parent.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub callee: Symbol,
    /// Abstract props at this call site (evaluated in parent's abstract env).
    pub props: HashMap<Symbol, StateValue>,
    pub location: Option<SourceRange>,
}

#[derive(Debug, Default, Clone)]
pub struct AnalysisStats {
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub recursion_cutoffs: usize,
    /// Number of components analyzed (including re-analyses due to fixpoint).
    pub components_analyzed: usize,
    /// (caller, callee) pairs where a recursive component reference was cut to ⊤.
    pub recursive_component_refs: HashSet<SymbolPair>,
    /// (caller, callee) pairs where the callee was not found in the registry.
    pub unknown_component_refs: HashSet<SymbolPair>,
    /// Components whose callback traversal hit the inline depth cap.
    pub callback_depth_capped: HashSet<Symbol>,
    /// Components where the utility-inlining splice budget
    /// (`Config::max_inline_depth`) ran out with calls still to inline, so
    /// those utility bodies stayed opaque (⊤).
    pub inline_budget_exhausted: HashSet<Symbol>,
}
