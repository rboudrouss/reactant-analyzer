pub mod cfg_builder;
pub mod component_detector;
pub mod expr_lower;
pub mod hook_detector;
pub mod hook_extractor;
pub mod import_resolution;
pub mod utility_detector;
pub mod utility_lowerer;

pub use crate::ir::{compute_line_starts, offset_to_range};
pub use cfg_builder::{build_cfg, build_fn_body_cfg};
pub use component_detector::{ComponentCandidate, detect_components};
pub use hook_detector::{HookCandidate, detect_custom_hooks};
pub use hook_extractor::{extract_handlers, extract_hooks, extract_subscriptions};
pub use import_resolution::build_resolved_import_map;
pub use utility_detector::{UtilityCandidate, detect_utilities};
pub use utility_lowerer::{lower_utilities, lower_utilities_with_resolver};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxc_ast::ast::{ImportDeclarationSpecifier, Program, Statement};

use crate::{
    ir::{component::ComponentIR, hook_ir::HookIR},
    resolver::{DefaultImportResolver, ImportResolver},
};

/// Build a map from locally-bound hook name → NPM package source for every
/// named import in `program`.
///
/// Example: `import { useQuery } from '@tanstack/react-query'`
///          → `{"useQuery": "@tanstack/react-query"}`
///
/// Only named and default imports are tracked; namespace imports (`* as foo`)
/// are skipped.  Relative imports (starting with `.`) are excluded they
/// are local files, not packages, and would not match SummaryRegistry entries.
fn build_import_map(program: &Program) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let source = decl.source.value.as_str();
        // Skip relative imports local files, not packages.
        if source.starts_with('.') {
            continue;
        }
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        for spec in specifiers {
            let local_name = match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => s.local.name.as_str(),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => s.local.name.as_str(),
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => continue,
            };
            if local_name.starts_with("use") {
                map.insert(local_name.to_string(), source.to_string());
            }
        }
    }
    map
}

/// Lower all user-defined custom hooks in `program` to `HookIR`.
/// Called alongside `lower_program` to build the `HookRegistry`.
///
/// `file` is the absolute path of the source file. It is stored on each
/// produced `HookIR` so the engine can key registries by `(file, name)` and
/// resolve cross-file imports.
///
/// Uses [`DefaultImportResolver`] for relative-import resolution. Callers that
/// need a custom resolver should use
/// [`lower_custom_hooks_with_resolver`].
pub fn lower_custom_hooks(program: &Program, line_starts: &[u32], file: &Path) -> Vec<HookIR> {
    lower_custom_hooks_with_resolver(program, line_starts, file, &DefaultImportResolver)
}

/// Plugin-friendly variant of [`lower_custom_hooks`] that accepts a custom
/// `ImportResolver`.
pub fn lower_custom_hooks_with_resolver(
    program: &Program,
    line_starts: &[u32],
    file: &Path,
    resolver: &dyn ImportResolver,
) -> Vec<HookIR> {
    let import_map = build_import_map(program);
    let resolved_import_map: HashMap<String, PathBuf> =
        build_resolved_import_map(program, file, resolver);
    detect_custom_hooks(program)
        .into_iter()
        .map(|candidate| {
            let (params, mut body_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, line_starts);
            let (mut hooks, mut next_label) =
                extract_hooks(&mut body_cfg, &import_map, &resolved_import_map);
            extract_handlers(&body_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            HookIR {
                file: file.to_path_buf(),
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
///
/// `file` is the absolute path of the source file. Uses
/// [`DefaultImportResolver`]; see [`lower_program_with_resolver`] for the
/// plugin variant.
pub fn lower_program(program: &Program, line_starts: &[u32], file: &Path) -> Vec<ComponentIR> {
    lower_program_with_resolver(program, line_starts, file, &DefaultImportResolver)
}

/// Plugin-friendly variant of [`lower_program`].
pub fn lower_program_with_resolver(
    program: &Program,
    line_starts: &[u32],
    file: &Path,
    resolver: &dyn ImportResolver,
) -> Vec<ComponentIR> {
    let import_map = build_import_map(program);
    let resolved_import_map: HashMap<String, PathBuf> =
        build_resolved_import_map(program, file, resolver);
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let (param_names, mut render_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, line_starts);
            let (mut hooks, mut next_label) =
                extract_hooks(&mut render_cfg, &import_map, &resolved_import_map);
            extract_handlers(&render_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            let param = param_names
                .into_iter()
                .next()
                .unwrap_or_else(|| "props".to_string());
            ComponentIR {
                file: file.to_path_buf(),
                name: candidate.name,
                param,
                render_cfg,
                hooks,
            }
        })
        .collect()
}
