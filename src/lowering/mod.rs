pub mod cfg_builder;
pub mod component_detector;

pub use cfg_builder::build_cfg;
pub use component_detector::{ComponentCandidate, detect_components};
