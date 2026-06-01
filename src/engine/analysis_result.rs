use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{cfg::CFG, expr::Expr, types::{BlockId, HookLabel, Var}},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    State,
    Effect,
    Memo,
    Callback,
    Ref,
    Custom,
}

/// Where a hook was called in the render CFG.
///
/// `block_id` is the block containing the hook's binding statement (StateVal,
/// MemoVal, CallbackVal) in the render CFG.  For Effect hooks, which emit no
/// statement in the render CFG, `block_id` defaults to the CFG entry.
#[derive(Debug, Clone)]
pub struct HookCallInfo {
    pub label: HookLabel,
    pub kind: HookKind,
    pub block_id: BlockId,
}

/// Captured information about a useEffect hook for dep-checking rules.
#[derive(Debug, Clone)]
pub struct EffectInfo {
    pub label: HookLabel,
    /// Variables used in the effect body but not locally defined within it.
    pub free_vars: HashSet<Var>,
    /// Deps array as declared by the caller (`[]` = empty, `None` = absent).
    pub declared_deps: Vec<Expr>,
}

#[derive(Debug)]
pub struct AnalysisResult<D: AbstractDomain> {
    pub state_store: StateStore<D>,
    pub memo_store: MemoStore<D>,
    /// Abstract environment at the *entry* of each render-CFG block.
    pub block_states: HashMap<BlockId, AbstractEnv<D>>,
    pub hook_calls: Vec<HookCallInfo>,
    pub effect_info: HashMap<HookLabel, EffectInfo>,
    /// Labels whose state was widened to force convergence.
    pub widened_labels: HashSet<HookLabel>,
    pub render_cfg: CFG,
}
