//! Lower top-level utility functions to [`FunctionIR`].
//!
//! Utilities are non-hook, non-component functions whose bodies the analyzer
//! would otherwise treat as opaque calls. By lowering them to a CFG, the
//! engine can inline them at statement-level call sites (`expand_utility_calls`).

use std::path::Path;

use oxc_ast::ast::Program;

use crate::{
    ir::{FileTable, FunctionIR, SourceMap},
    lowering::{LowerCtx, build_jsx_origins, utility_detector::detect_utilities},
    resolver::{DefaultImportResolver, ImportResolver},
};

/// Lower every top-level utility function in `program`, attaching `file` to
/// each [`FunctionIR`] so registry keys remain `(file, name)`-unique.
///
/// `source` is the file's text (for the span line table); `files` interns
/// `file` so every produced span carries its [`crate::ir::FileId`] (ADR-019).
pub fn lower_utilities(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
) -> Vec<FunctionIR> {
    lower_utilities_with_resolver(
        program,
        source,
        file,
        files,
        &DefaultImportResolver::default(),
    )
}

/// Plugin-friendly variant of [`lower_utilities`].
pub fn lower_utilities_with_resolver(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
    resolver: &dyn ImportResolver,
) -> Vec<FunctionIR> {
    // One context per file — see `LowerCtx`.
    let ctx = LowerCtx::new(
        SourceMap::new(source, files.intern(file)),
        build_jsx_origins(program, file, resolver),
    );
    detect_utilities(program)
        .into_iter()
        .map(|candidate| {
            let (params, body_cfg) = candidate.build_cfg(&ctx);
            FunctionIR {
                file: file.to_path_buf(),
                name: candidate.name,
                params,
                body_cfg,
            }
        })
        .collect()
}
