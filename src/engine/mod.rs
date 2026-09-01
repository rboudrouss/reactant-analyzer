pub mod analysis_result;
pub mod cfg_analyzer;
pub mod component_cache;
pub mod component_registry;
pub mod dominance;
pub mod fixpoint;
pub mod function_registry;
pub mod hook_registry;
pub mod program_result;
pub mod root_detector;
pub mod setters;
pub mod symbol_graph;

pub use analysis_result::{
    AnalysisResult, EffectInfo, HandlerInfo, HookCallInfo, HookKind, InlineKind, InlineOrigin,
    WidenEvent,
};
pub use cfg_analyzer::analyze_cfg;
pub use component_cache::ComponentCache;
pub use component_registry::{ComponentKey, ComponentRegistry};
pub use dominance::{DominatorTree, compute_dominators, dominates, rpo};
pub use fixpoint::{Config, analyze_component, analyze_component_inter, analyze_program};
pub use function_registry::{FunctionKey, FunctionRegistry};
pub use hook_registry::{HookKey, HookRegistry};
pub use program_result::{AnalysisStats, CallSite, ComponentCallGraph, ProgramAnalysisResult};
pub use root_detector::RootStrategy;
pub use setters::{SlotWriter, WriterPhase, WriterRegion};
pub use symbol_graph::{SymbolGraph, SymbolKind, SymbolNode};
