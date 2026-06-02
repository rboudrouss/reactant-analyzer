pub mod cfg_builder;
pub mod component_detector;
pub mod expr_lower;
pub mod hook_extractor;

pub use cfg_builder::build_cfg;
pub use component_detector::{ComponentCandidate, detect_components};
pub use hook_extractor::extract_hooks;

use oxc_ast::ast::Program;

use crate::ir::component::ComponentIR;

/// Stage 3 entry point: lower all React components in `program` to `ComponentIR`.
pub fn lower_program(program: &Program) -> Vec<ComponentIR> {
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let mut render_cfg = build_cfg(candidate.body);
            let hooks = extract_hooks(&mut render_cfg);
            let params = expr_lower::lower_params(candidate.params);
            let param = params
                .into_iter()
                .next()
                .unwrap_or_else(|| "props".to_string());
            ComponentIR {
                name: candidate.name,
                param,
                render_cfg,
                hooks,
            }
        })
        .collect()
}
