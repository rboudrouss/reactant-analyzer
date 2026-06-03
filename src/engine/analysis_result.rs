use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{
        cfg::{CFG, Terminator},
        expr::Expr,
        hooks::HookEntry,
        types::{BlockId, HookLabel, Var},
    },
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
    /// `true` when caller wrote an explicit deps array (even `[]`).
    /// `false` when no deps argument was passed (`deps: None`).
    pub has_deps_array: bool,
}

#[derive(Debug)]
pub struct AnalysisResult<D: AbstractDomain> {
    pub state_store: StateStore<D>,
    pub memo_store: MemoStore<D>,
    /// Abstract environment at the *exit* of each render-CFG block.
    pub block_states: HashMap<BlockId, AbstractEnv<D>>,
    /// Abstract environment at the *exit* of each block, per effect body CFG.
    /// Populated at convergence (overwritten each iteration; last write is final).
    pub effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>,
    pub hook_calls: Vec<HookCallInfo>,
    pub effect_info: HashMap<HookLabel, EffectInfo>,
    /// Labels whose state was widened to force convergence.
    pub widened_labels: HashSet<HookLabel>,
    pub render_cfg: CFG,
    /// Original hook entries — needed by rules that inspect effect body CFGs.
    pub hooks: Vec<HookEntry>,
}

impl<D: AbstractDomain> AnalysisResult<D> {
    /// Join the abstract exit envs of all `Return`-terminated blocks.
    ///
    /// Uses `reduce` (not `fold(bottom, join)`) since `bottom.join(env)` maps
    /// any key not in `bottom` to `D::top()`, making bottom a non-identity.
    pub fn exit_env(&self) -> AbstractEnv<D> {
        self.render_cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .filter_map(|b| self.block_states.get(&b.id))
            .cloned()
            .reduce(|acc, env| acc.join(&env))
            .unwrap_or_else(AbstractEnv::bottom)
    }
}
