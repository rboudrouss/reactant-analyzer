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

use std::collections::HashMap;

use oxc_ast::ast::{ImportDeclarationSpecifier, Program, Statement};

use crate::ir::{component::ComponentIR, hook_ir::HookIR};

/// Build a map from locally-bound hook name → NPM package source for every
/// named import in `program`.
///
/// Example: `import { useQuery } from '@tanstack/react-query'`
///          → `{"useQuery": "@tanstack/react-query"}`
///
/// Only named and default imports are tracked; namespace imports (`* as foo`)
/// are skipped.  Relative imports (starting with `.`) are excluded — they
/// are local files, not packages, and would not match SummaryRegistry entries.
fn build_import_map(program: &Program) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let source = decl.source.value.as_str();
        // Skip relative imports — local files, not packages.
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
pub fn lower_custom_hooks(program: &Program, line_starts: &[u32]) -> Vec<HookIR> {
    let import_map = build_import_map(program);
    detect_custom_hooks(program)
        .into_iter()
        .map(|candidate| {
            let (params, mut body_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, line_starts);
            let (mut hooks, mut next_label) = extract_hooks(&mut body_cfg, &import_map);
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
    let import_map = build_import_map(program);
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let (param_names, mut render_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, line_starts);
            let (mut hooks, mut next_label) = extract_hooks(&mut render_cfg, &import_map);
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
