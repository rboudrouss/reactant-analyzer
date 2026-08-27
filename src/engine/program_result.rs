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
}
