//! Lower top-level utility functions to [`FunctionIR`] (ADR-013 Phase 3).
//!
//! Utilities are non-hook, non-component functions whose bodies the analyzer
//! would otherwise treat as opaque calls. By lowering them to a CFG, the
//! engine can inline them at statement-level call sites (`expand_utility_calls`).

use std::path::Path;

use oxc_ast::ast::Program;

use crate::{
    ir::FunctionIR,
    lowering::{cfg_builder::build_fn_body_cfg, utility_detector::detect_utilities},
    resolver::{DefaultImportResolver, ImportResolver},
};

/// Lower every top-level utility function in `program`, attaching `file` to
/// each [`FunctionIR`] so registry keys remain `(file, name)`-unique.
pub fn lower_utilities(program: &Program, line_starts: &[u32], file: &Path) -> Vec<FunctionIR> {
    lower_utilities_with_resolver(program, line_starts, file, &DefaultImportResolver)
}

/// Plugin-friendly variant of [`lower_utilities`] (ADR-013 §2 + Phase 4).
pub fn lower_utilities_with_resolver(
    program: &Program,
    line_starts: &[u32],
    file: &Path,
    _resolver: &dyn ImportResolver,
) -> Vec<FunctionIR> {
    // Currently utilities don't carry hook entries or resolved imports — the
    // resolver argument is reserved for future use (e.g. inlining transitive
    // utility imports). Kept for API symmetry with `lower_program`.
    detect_utilities(program)
        .into_iter()
        .map(|candidate| {
            let (params, body_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, line_starts);
            FunctionIR {
                file: file.to_path_buf(),
                name: candidate.name,
                params,
                body_cfg,
            }
        })
        .collect()
}
