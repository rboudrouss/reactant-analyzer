pub mod analysis_result;
pub mod cfg_analyzer;
pub mod component_cache;
pub mod component_registry;
pub mod dominance;
pub mod fixpoint;
pub mod program_result;
pub mod root_detector;

pub use analysis_result::{AnalysisResult, EffectInfo, HandlerInfo, HookCallInfo, HookKind};
pub use cfg_analyzer::analyze_cfg;
pub use component_cache::ComponentCache;
pub use component_registry::ComponentRegistry;
pub use dominance::{compute_dominators, dominates, rpo};
pub use fixpoint::{Config, analyze_component, analyze_component_inter, analyze_program};
pub use program_result::{AnalysisStats, CallSite, ComponentCallGraph, ProgramAnalysisResult};
pub use root_detector::RootStrategy;
