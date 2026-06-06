//! Map each locally-bound import name to the file it resolves to.
//!
//! Mirror of [`crate::lowering::build_import_map`] for **relative** imports
//! only. Where `build_import_map` records the npm package source for a name
//! (`"useQuery" → "@tanstack/react-query"`), this module records the absolute
//! file path that a relative specifier resolves to via [`ImportResolver`].
//!
//! Populated values flow into [`crate::ir::hooks::HookEntry::Custom::resolved_file`]
//! at extraction time (ADR-013 §1 + §2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxc_ast::ast::{ImportDeclarationSpecifier, Program, Statement};

use crate::resolver::ImportResolver;

/// Build a map from locally-bound import name → resolved absolute file path,
/// for every **relative** import in `program`.
///
/// Examples:
///   - `import { ChildPage } from './child'` with `./child.tsx` present
///     → `{"ChildPage": "/abs/.../child.tsx"}`
///   - `import { useData } from './hooks/useData'` with `./hooks/useData.ts`
///     → `{"useData": "/abs/.../hooks/useData.ts"}`
///   - `import { useQuery } from '@tanstack/react-query'` (non-relative)
///     → not included (handled by [`crate::lowering::build_import_map`])
///   - Unresolvable specifiers → silently omitted; the engine then sees
///     `resolved_file: None` and falls back to legacy name-only lookup.
///
/// `current_file` is the path of the importing file, used as the base for
/// relative resolution.
pub fn build_resolved_import_map(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let source = decl.source.value.as_str();
        // Only relative imports — npm packages are tracked by build_import_map.
        if !source.starts_with('.') {
            continue;
        }
        let Some(resolved) = resolver.resolve(current_file, source) else {
            continue;
        };
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        for spec in specifiers {
            let local_name = match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => s.local.name.as_str(),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => s.local.name.as_str(),
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => continue,
            };
            map.insert(local_name.to_string(), resolved.clone());
        }
    }
    map
}
