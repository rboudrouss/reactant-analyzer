pub mod cfg_builder;
pub mod component_detector;
pub mod expr_lower;
pub mod hook_detector;
pub mod hook_extractor;

pub use crate::ir::{compute_line_starts, offset_to_range};
pub use cfg_builder::{build_cfg, build_fn_body_cfg};
pub use component_detector::{ComponentCandidate, detect_components};
pub use hook_detector::{HookCandidate, detect_custom_hooks};
pub use hook_extractor::{extract_handlers, extract_hooks, extract_subscriptions};

use oxc_ast::ast::Program;

use crate::ir::{component::ComponentIR, hook_ir::HookIR};

/// Lower all user-defined custom hooks in `program` to `HookIR`.
/// Called alongside `lower_program` to build the `HookRegistry`.
pub fn lower_custom_hooks(program: &Program, line_starts: &[u32]) -> Vec<HookIR> {
    detect_custom_hooks(program)
        .into_iter()
        .map(|candidate| {
            let (params, mut body_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, line_starts);
            let (mut hooks, mut next_label) = extract_hooks(&mut body_cfg);
            extract_handlers(&body_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            HookIR {
                name: candidate.name,
                params,
                body_cfg,
                hooks,
                next_label,
            }
        })
        .collect()
}

/// Stage 3 entry point: lower all React components in `program` to `ComponentIR`.
pub fn lower_program(program: &Program, line_starts: &[u32]) -> Vec<ComponentIR> {
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let (param_names, mut render_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, line_starts);
            let (mut hooks, mut next_label) = extract_hooks(&mut render_cfg);
            extract_handlers(&render_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            let param = param_names
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
