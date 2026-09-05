use std::collections::{HashMap, HashSet};

use crate::{
    domains::{impls::StateValue, stores::SharedStateStore},
    engine::analysis_result::AnalysisResult,
    ir::{
        ComponentId, ComponentTable, ModuleTable,
        source_range::{FileTable, SourceRange},
        types::Symbol,
    },
};

/// A `(caller, callee)` pair, as the stats sets record them.
pub type ComponentPair = (ComponentId, ComponentId);

/// A JSX callee the analysis could not pin to a component: the body that wrote
/// it, and the name as written there. The callee half is a bare name for the
/// same reason the row exists — nothing resolved it to a [`ComponentId`].
pub type UnresolvedRef = (ComponentId, Symbol);

/// Program-level analysis result spanning all components.
/// Rules receive `&ProgramAnalysisResult` and access per-component data via `components`.
///
/// `Default` is the empty program: every side table answers "nothing known",
/// which is the reading each of their doc comments already prescribes. It lets
/// a caller assembling a program by hand fill in the two fields it has —
/// `components` and `component_table` — without restating the rest.
#[derive(Debug, Default)]
pub struct ProgramAnalysisResult {
    pub components: HashMap<ComponentId, AnalysisResult<StateValue>>,
    pub shared_state: SharedStateStore,
    pub call_graph: ComponentCallGraph,
    /// Components whose recursion was cut off (received ⊤ result).
    pub recursive_components: HashSet<ComponentId>,
    pub stats: AnalysisStats,
    /// Resolves the [`ComponentId`] every table above is keyed by, and mints
    /// the display name a report shows (#7). Empty when the IR was built by
    /// hand (unit tests), whose single component is
    /// [`ComponentId::SYNTHETIC`].
    pub component_table: ComponentTable,
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
    pub phase1_reached: HashSet<ComponentId>,
}

impl ProgramAnalysisResult {
    /// The component this result shows as `name`, when exactly one does.
    ///
    /// The public inverse of [`Self::display_name`]: a report prints a name
    /// and a caller — `--entry`, a test, an embedder — hands it back. `None`
    /// when nothing wears that name, and `None` when several do and the bare
    /// form cannot say which, in which case pass the qualified `Name@file`
    /// form the report prints.
    pub fn component_named(&self, name: &str) -> Option<ComponentId> {
        self.component_table.resolve_display_name(name)
    }

    /// A program of one already-analysed component, shown as `name`.
    ///
    /// What [`crate::engine::analyze_program`] does for a whole registry, for
    /// a caller that analysed a single component with
    /// [`crate::engine::analyze_component`]. The identity the result already
    /// carries is registered as-is rather than re-minted: that result stamped
    /// it into its own state labels and setter owners, and a fresh id would
    /// make every one of those lookups miss (#7).
    pub fn single(name: &str, result: AnalysisResult<StateValue>) -> Self {
        let mut component_table = ComponentTable::default();
        component_table.register(
            result.component,
            crate::ir::CompOrigin {
                file: result.file.clone(),
                name: name.to_string(),
            },
        );
        let mut components = HashMap::new();
        components.insert(result.component, result);
        ProgramAnalysisResult {
            components,
            component_table,
            shared_state: Default::default(),
            call_graph: ComponentCallGraph::new(),
            recursive_components: HashSet::new(),
            stats: AnalysisStats::default(),
            file_table: Default::default(),
            module_table: Default::default(),
            function_registry: Default::default(),
            phase1_reached: Default::default(),
        }
    }

    /// Was `comp` analysed top-down in phase 1, with its callers' props and
    /// callbacks flowed in?
    ///
    /// `false` means one of two things the analysis cannot tell apart — the
    /// component is genuinely unreachable, or nothing that renders it was a
    /// root — so a consumer must treat its ancestry as unknown rather than
    /// empty (#110, #20).
    pub fn was_inter_analyzed(&self, comp: ComponentId) -> bool {
        self.phase1_reached.contains(&comp)
    }

    /// The transitive caller closure of `comp`, or `None` when any component on
    /// the way up was not inter-analysed — the ancestry is then unknown, not
    /// empty, and a caller reasoning about absence must not proceed.
    ///
    /// Cycle-safe: a recursive component's closure includes itself and
    /// terminates. `comp` itself must be inter-analysed, otherwise even its
    /// direct callers are unknown.
    pub fn complete_ancestry(&self, comp: ComponentId) -> Option<HashSet<ComponentId>> {
        if !self.was_inter_analyzed(comp) {
            return None;
        }
        let mut seen: HashSet<ComponentId> = HashSet::new();
        let mut queue = vec![comp];
        while let Some(cur) = queue.pop() {
            for caller in self.call_graph.callers_of(cur) {
                if !self.was_inter_analyzed(caller) {
                    return None;
                }
                if seen.insert(caller) {
                    queue.push(caller);
                }
            }
        }
        Some(seen)
    }

    /// The name to show for `comp` — the whole reason the table travels with
    /// the result.
    ///
    /// Every id the analysis mints is interned, so the fallback is a bug
    /// rather than a case: it prints the raw index so the id is at least
    /// traceable instead of silently becoming an empty name.
    pub fn display_name(&self, comp: ComponentId) -> String {
        self.component_table
            .display_name(comp)
            .unwrap_or_else(|| format!("component#{}", comp.index()))
    }
}

/// Directed call graph: caller → list of call sites.
#[derive(Debug, Default, Clone)]
pub struct ComponentCallGraph {
    pub edges: HashMap<ComponentId, Vec<CallSite>>,
}

impl ComponentCallGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_edge(&mut self, caller: ComponentId, site: CallSite) {
        self.edges.entry(caller).or_default().push(site);
    }

    pub fn callees_of(&self, comp: ComponentId) -> &[CallSite] {
        self.edges.get(&comp).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn callers_of(&self, comp: ComponentId) -> Vec<ComponentId> {
        self.edges
            .iter()
            .filter(|(_, sites)| sites.iter().any(|s| s.callee == comp))
            .map(|(caller, _)| *caller)
            .collect()
    }
}

/// One instantiation of a child component inside a parent.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub callee: ComponentId,
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
    pub recursive_component_refs: HashSet<ComponentPair>,
    /// (caller, callee) pairs where the callee was not found in the registry.
    pub unknown_component_refs: HashSet<UnresolvedRef>,
    /// (caller, callee) pairs where several analysed files define the callee's
    /// name and nothing at the call site says which one is meant (#7). The
    /// child is treated as unanalysable, exactly like an unknown one — the
    /// two are kept apart only because the user's remedy differs.
    pub ambiguous_component_refs: HashSet<UnresolvedRef>,
    /// Components whose callback traversal hit the inline depth cap.
    pub callback_depth_capped: HashSet<ComponentId>,
    /// Components where the utility-inlining splice budget
    /// (`Config::max_inline_depth`) ran out with calls still to inline, so
    /// those utility bodies stayed opaque (⊤).
    pub inline_budget_exhausted: HashSet<ComponentId>,
}
