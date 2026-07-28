//! Map each locally-bound import name to the file it resolves to.
//!
//! Mirror of [`crate::lowering::build_import_map`] for **relative** imports
//! only. Where `build_import_map` records the npm package source for a name
//! (`"useQuery" → "@tanstack/react-query"`), this module records the absolute
//! file path that a relative specifier resolves to via [`ImportResolver`].
//!
//! Populated values flow into [`crate::ir::hooks::HookEntry::Custom::resolved_file`]
//! at extraction time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxc_ast::ast::{ImportDeclarationSpecifier, Program, Statement};

use crate::resolver::ImportResolver;

/// A relative import resolved to its defining file, keeping the name the
/// *origin* exports under.
///
/// The distinction matters whenever a fact is looked up in the origin file's
/// tables: `import { Ctx as C } from "./ctx"` binds `C` here but `./ctx`
/// knows it as `Ctx`, so a local-name-only map cannot find it. (The same
/// missing field is what makes `import { useMemo as useM }` classify as a
/// custom hook — see `docs/TODO.md`.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImport {
    /// Absolute path the specifier resolved to.
    pub file: PathBuf,
    /// Name the origin file exports it under — the local name for a default
    /// import, which has no exported name of its own.
    pub imported: String,
}

/// Build a map from locally-bound import name → resolved origin, for every
/// **relative** import in `program`. [`build_resolved_import_map`] is the
/// file-only projection of this.
pub fn build_resolved_imports(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> HashMap<String, ResolvedImport> {
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
            let (local, imported) = match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    (s.local.name.as_str(), s.imported.name().to_string())
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (s.local.name.as_str(), s.local.name.to_string())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => continue,
            };
            map.insert(
                local.to_string(),
                ResolvedImport {
                    file: resolved.clone(),
                    imported,
                },
            );
        }
    }
    map
}

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
    build_resolved_imports(program, current_file, resolver)
        .into_iter()
        .map(|(local, r)| (local, r.file))
        .collect()
}
