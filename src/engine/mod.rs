pub mod analysis_result;
pub mod cfg_analyzer;
pub mod dominance;
pub mod fixpoint;

pub use analysis_result::{AnalysisResult, EffectInfo, HookCallInfo, HookKind};
pub use cfg_analyzer::analyze_cfg;
pub use dominance::{compute_dominators, dominates, rpo};
pub use fixpoint::{Config, analyze_component};
