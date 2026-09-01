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

/// [`ProgramAnalysisResult`] wrapping a single component `name → result`, with
/// every side table at its default value.
pub(crate) fn prog(name: &str, result: AnalysisResult<StateValue>) -> ProgramAnalysisResult {
    let mut components = HashMap::new();
    components.insert(name.to_string(), result);
    ProgramAnalysisResult {
        components,
        shared_state: SharedStateStore::default(),
        call_graph: ComponentCallGraph::new(),
        recursive_components: HashSet::new(),
        stats: AnalysisStats::default(),
        file_table: Default::default(),
        module_table: Default::default(),
        function_registry: Default::default(),
    }
}

/// Default [`AnalysisResult<StateValue>`] for component `"C"` with the given
/// render CFG and every other field at its empty/default value. Tests that vary
/// additional fields use struct-update syntax:
/// `AnalysisResult { hooks, ..analysis_result(cfg) }`.
pub(crate) fn analysis_result(render_cfg: CFG) -> AnalysisResult<StateValue> {
    AnalysisResult {
        component: "C".to_string(),
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
        custom_arg_returns: HashMap::new(),
        iterations: 0,
        effect_setter_writes: StateStore::bottom(),
        heap: Heap::new(),
    }
}
