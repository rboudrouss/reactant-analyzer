use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{
    domains::{AbstractDomain, impls::StateValue, stores::SharedStateStore},
    engine::analysis_result::AnalysisResult,
    ir::{
        ComponentId, ComponentTable, ModuleTable,
        source_range::{FileTable, SourceRange},
        types::{HookLabel, Symbol},
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

/// Directed call graph: caller → its call sites, one row per element.
#[derive(Debug, Default, Clone)]
pub struct ComponentCallGraph {
    pub edges: HashMap<ComponentId, Vec<CallSite>>,
    /// Callee → `(caller, index in edges[caller])`, the reverse index.
    by_callee: HashMap<ComponentId, Vec<(ComponentId, usize)>>,
}

impl ComponentCallGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one evaluation of an element. An element is evaluated once per
    /// pass of its caller's fixpoint and once per input the caller is
    /// analysed under, so a second evaluation of the same `(callee,
    /// location)` joins its props into the row.
    pub fn add_edge(&mut self, caller: ComponentId, site: CallSite) {
        let rows = self.edges.entry(caller).or_default();
        if let Some(row) = rows
            .iter_mut()
            .find(|r| r.callee == site.callee && r.location == site.location)
        {
            let absent = StateValue::undefined();
            for (k, v) in site.props {
                let joined = row.props.get(&k).unwrap_or(&absent).join(&v);
                row.props.insert(k, joined);
            }
            return;
        }
        self.by_callee
            .entry(site.callee)
            .or_default()
            .push((caller, rows.len()));
        rows.push(site);
    }

    pub fn callees_of(&self, comp: ComponentId) -> &[CallSite] {
        self.edges.get(&comp).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn callers_of(&self, comp: ComponentId) -> Vec<ComponentId> {
        let mut callers: Vec<ComponentId> = self
            .by_callee
            .get(&comp)
            .into_iter()
            .flatten()
            .map(|(caller, _)| *caller)
            .collect();
        callers.sort();
        callers.dedup();
        callers
    }

    /// Every analysed element that renders `comp`, with the component whose
    /// render builds it.
    pub fn sites_of(&self, comp: ComponentId) -> impl Iterator<Item = (ComponentId, &CallSite)> {
        self.by_callee
            .get(&comp)
            .into_iter()
            .flatten()
            .map(|(caller, i)| (*caller, &self.edges[caller][*i]))
    }

    /// The deps of `hook` in `comp` that are a new reference at every render
    /// under one element's input alone, each with the components whose
    /// render builds such an element (#175, #237). What the joined result
    /// loses: a reader quantifying "always fresh" over a prop-derived dep
    /// reads it where the result does not say so, and names the parents.
    pub fn fresh_through(
        &self,
        comp: ComponentId,
        hook: HookLabel,
    ) -> BTreeMap<usize, BTreeSet<ComponentId>> {
        let mut out: BTreeMap<usize, BTreeSet<ComponentId>> = BTreeMap::new();
        for (caller, site) in self.sites_of(comp) {
            let fresh = site.own.as_ref().and_then(|o| o.fresh_deps.get(&hook));
            for &i in fresh.into_iter().flatten() {
                out.entry(i).or_default().insert(caller);
            }
        }
        out
    }

    /// The row of one element, to record what the callee does under it.
    pub fn site_mut(
        &mut self,
        caller: ComponentId,
        callee: ComponentId,
        location: Option<SourceRange>,
    ) -> Option<&mut CallSite> {
        self.edges
            .get_mut(&caller)?
            .iter_mut()
            .find(|r| r.callee == callee && r.location == location)
    }
}

/// The deps of a component's hooks that are a new reference at every render:
/// hook → indices in its deps list.
pub type FreshDeps =
    std::collections::BTreeMap<crate::ir::types::HookLabel, std::collections::BTreeSet<usize>>;

/// One element instantiating a child component inside a parent.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub callee: ComponentId,
    /// Abstract props at this element: the join of every evaluation of it.
    pub props: HashMap<Symbol, StateValue>,
    /// The element's span; `None` for an element lowering could not place,
    /// all of which share one row per callee.
    pub location: Option<SourceRange>,
    /// What the callee does when this element alone renders it, where that
    /// differs from its result. The callee's entry in the results is its
    /// analysis under the join of every site's input (#175), and a fact that
    /// holds at one site — a dep that is a new reference at every render —
    /// is lost in that join. `None`: no other input reaches the callee, or
    /// this one was not re-analysed.
    pub own: Option<SiteFacts>,
}

/// The callee's facts under one call site's input alone (must, for that
/// site).
#[derive(Debug, Clone, Default)]
pub struct SiteFacts {
    pub fresh_deps: FreshDeps,
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
