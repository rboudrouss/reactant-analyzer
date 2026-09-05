//! Shared `#[cfg(test)]` fixture builders.
//!
//! Consolidates boilerplate that was duplicated across rule / engine / ir test
//! modules: the one-block render CFG, the single-component
//! [`ProgramAnalysisResult`], and the default [`AnalysisResult`]. Compiled only
//! for tests (the module is gated behind `#[cfg(test)]` in `lib.rs`).

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        impls::StateValue,
        stores::{Heap, MemoStore, SharedStateStore, StateStore},
    },
    engine::{
        AnalysisResult,
        program_result::{AnalysisStats, ComponentCallGraph, ProgramAnalysisResult},
    },
    ir::{
        cfg::{BasicBlock, CFG, Terminator},
        expr::{Expr, Prim},
        stmt::Stmt,
    },
};

/// One-block render CFG terminated by `return unit` — the canonical trivial
/// body used by most rule/engine tests (entry 0, block id 0, no edges).
pub(crate) fn single_block_cfg(stmts: Vec<Stmt>) -> CFG {
    single_block_cfg_term(stmts, Terminator::Return(Expr::Lit(Prim::Unit)))
}

/// One-block CFG with a caller-chosen terminator (entry 0, block id 0, no edges).
pub(crate) fn single_block_cfg_term(stmts: Vec<Stmt>, term: Terminator) -> CFG {
    let mut blocks = std::collections::BTreeMap::new();
    blocks.insert(0, BasicBlock { id: 0, stmts, term });
    CFG {
        entry: 0,
        blocks,
        edges: vec![],
    }
}

/// The id a hand-built [`analysis_result`] carries, and therefore the one
/// [`prog`] registers it under — what a test passes to
/// [`crate::rules::RuleCtx::new`].
pub(crate) const C: crate::ir::ComponentId = crate::ir::ComponentId::SYNTHETIC;

/// A second, third … distinct component, for a fixture that needs more than
/// one identity and builds no table.
pub(crate) const fn cid(i: u32) -> crate::ir::ComponentId {
    crate::ir::ComponentId::from_index(i)
}

/// Interns a fixture component *name*, the way `ComponentTable` interns a real
/// one — so a unit test can go on naming its components while the code under
/// test compares ids. Process-wide and monotone: one name is one id for the
/// life of the test binary.
pub(crate) fn named(name: &str) -> crate::ir::ComponentId {
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("fixture name table");
    if let Some(i) = names.iter().position(|n| n == name) {
        return cid(i as u32);
    }
    names.push(name.to_string());
    cid((names.len() - 1) as u32)
}

/// [`ProgramAnalysisResult`] wrapping a single component, with every side
/// table at its default value.
///
/// Interns `name` in a real [`crate::ir::ComponentTable`] and stamps the id it
/// gets onto the result, exactly as `ComponentRegistry` does in production —
/// so a fixture never carries an identity the table cannot resolve, and
/// `display_name` answers with the name the test asked for.
pub(crate) fn prog(name: &str, result: AnalysisResult<StateValue>) -> ProgramAnalysisResult {
    // The result may already carry an identity — `analyze_component` stamps
    // `SYNTHETIC` into its state labels and setter owners — and re-keying the
    // map without those would make every owner lookup miss. So the table takes
    // the id the result has.
    let mut component_table = crate::ir::ComponentTable::default();
    let id = result.component;
    component_table.register(
        id,
        crate::ir::CompOrigin {
            file: result.file.clone(),
            name: name.to_string(),
        },
    );
    let mut components = HashMap::new();
    components.insert(id, result);
    ProgramAnalysisResult {
        components,
        component_table,
        shared_state: SharedStateStore::default(),
        call_graph: ComponentCallGraph::new(),
        recursive_components: HashSet::new(),
        stats: AnalysisStats::default(),
        file_table: Default::default(),
        module_table: Default::default(),
        function_registry: Default::default(),
        phase1_reached: Default::default(),
    }
}

/// Default [`AnalysisResult<StateValue>`] for component `"C"` with the given
/// render CFG and every other field at its empty/default value. Tests that vary
/// additional fields use struct-update syntax:
/// `AnalysisResult { hooks, ..analysis_result(cfg) }`.
pub(crate) fn analysis_result(render_cfg: CFG) -> AnalysisResult<StateValue> {
    AnalysisResult {
        component: crate::ir::ComponentId::SYNTHETIC,
        file: Default::default(),
        param: "props".to_string(),
        dom_props: Default::default(),
        module_consts: Default::default(),
        state_store: StateStore::bottom(),
        memo_store: MemoStore::new(),
        block_states: HashMap::new(),
        effect_block_states: HashMap::new(),
        hook_calls: vec![],
        effect_info: HashMap::new(),
        handler_block_states: HashMap::new(),
        handler_info: HashMap::new(),
        widen_trace: HashMap::new(),
        inline_origins: Vec::new(),
        render_cfg,
        hooks: vec![],
        hook_provenance: vec![],
        slot_writers: vec![],
        slot_seeds: vec![],
        registrations: vec![],
        custom_arg_returns: HashMap::new(),
        iterations: 0,
        effect_setter_writes: StateStore::bottom(),
        heap: Heap::new(),
    }
}
