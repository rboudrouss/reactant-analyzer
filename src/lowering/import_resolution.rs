//! Map each locally-bound import name to what its import declaration proves.
//!
//! Two relations live here: [`build_hook_origins`] classifies hook-relevant
//! bindings by provenance (ADR-023 step 1) and feeds hook extraction;
//! [`build_resolved_imports`] resolves every import the project's resolver
//! maps to a real file, keeping the name the origin exports under.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc_ast::ast::{ImportDeclarationSpecifier, Program, Statement};

use crate::ir::expr::{CompOrigin, ElementOrigin};
use crate::ir::types::Symbol;
use crate::resolver::ImportResolver;

/// An import resolved to its defining file, keeping the name the *origin*
/// exports under.
///
/// The distinction matters whenever a fact is looked up in the origin file's
/// tables: `import { Ctx as C } from "./ctx"` binds `C` here but `./ctx`
/// knows it as `Ctx`, so a local-name-only map cannot find it. (The same
/// missing field is what made `import { useMemo as useM }` classify as a
/// custom hook, before ADR-023 step 1 closed it.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImport {
    /// Absolute path the specifier resolved to.
    pub file: PathBuf,
    /// Name the origin file exports it under — the local name for a default
    /// import, which has no exported name of its own.
    pub imported: String,
}

/// Provenance of one hook-relevant imported binding, decided fail-closed:
/// each variant records what was *proven* about the origin, and the raw
/// specifier is retained on every non-React variant so a package-scoped
/// lookup ([`crate::registry::SummaryRegistry`]) still matches when a
/// self-aliasing tsconfig path resolves the package to a local file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOrigin {
    /// The literal specifier was `"react"` — decided *before* the resolver is
    /// consulted, so a project aliasing `react` to a file keeps React's hooks
    /// classified as React's (ADR-023 step 1: the mass-FN hazard).
    React { imported: String },
    /// The specifier resolved to a local file. `specifier` is the raw import
    /// text (`"zustand"` under a self-alias, `"./hooks/useData"`).
    File {
        file: PathBuf,
        specifier: String,
        imported: String,
    },
    /// The specifier did not resolve — an npm package, or a missing file.
    Package { specifier: String, imported: String },
}

impl HookOrigin {
    /// Name the origin exports the binding under (alias-resolved).
    pub fn imported(&self) -> &str {
        match self {
            HookOrigin::React { imported }
            | HookOrigin::File { imported, .. }
            | HookOrigin::Package { imported, .. } => imported,
        }
    }
}

/// Build a map from locally-bound import name → [`HookOrigin`], for every
/// import specifier whose local *or* imported name follows the hook naming
/// rule. Keying on either side is what makes a call through a non-`use`
/// binding (`import { useThing as thing }`) still classify as a hook row,
/// and an aliased React hook (`import { useMemo as useM } from "react"`)
/// classify as React's.
pub fn build_hook_origins(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> HashMap<String, HookOrigin> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        let source = decl.source.value.as_str();
        // The literal specifier decides React-ness BEFORE the resolver runs:
        // a self-aliasing tsconfig `paths` entry mapping "react" to a file
        // must not demote React's hooks to opaque Custom rows.
        let is_react = source == "react";
        // Resolved lazily: only consulted when a hook-relevant specifier
        // exists on a non-react declaration.
        let mut resolved: Option<Option<PathBuf>> = None;
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
            if !(super::is_hook_name(local) || super::is_hook_name(&imported)) {
                continue;
            }
            let origin = if is_react {
                HookOrigin::React { imported }
            } else {
                let file = resolved
                    .get_or_insert_with(|| resolver.resolve(current_file, source))
                    .clone();
                match file {
                    Some(file) => HookOrigin::File {
                        file,
                        specifier: source.to_string(),
                        imported,
                    },
                    None => HookOrigin::Package {
                        specifier: source.to_string(),
                        imported,
                    },
                }
            };
            map.insert(local.to_string(), origin);
        }
    }
    map
}

/// Build a map from locally-bound import name → resolved origin, for every
/// import in `program` the resolver maps to a real file — relative or aliased.
/// [`build_resolved_import_map`] is the file-only projection of this.
///
/// No relative-only pre-filter: the resolver answers `None` for anything it
/// cannot map to an existing source file, so admitting non-relative
/// specifiers can only add edges an alias made resolvable (`@/lib/ctx` in a
/// Vite or Next project), never redirect one that already resolved.
pub fn build_resolved_imports(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> HashMap<String, ResolvedImport> {
    import_bindings(program, current_file, resolver)
        .into_iter()
        .filter_map(|(local, resolved)| Some((local, resolved?)))
        .collect()
}

/// Every named and default import binding of `program`, with the definition
/// it resolves to — `None` when the resolver maps its specifier to no file.
fn import_bindings(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> Vec<(String, Option<ResolvedImport>)> {
    let mut out = Vec::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        let resolved = resolver.resolve(current_file, decl.source.value.as_str());
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
            out.push((
                local.to_string(),
                resolved
                    .clone()
                    .map(|file| ResolvedImport { file, imported }),
            ));
        }
    }
    out
}

/// Build a map from locally-bound import name → resolved absolute file path,
/// for every import in `program` that resolves to an analyzed file.
///
/// Examples:
///   - `import { ChildPage } from './child'` with `./child.tsx` present
///     → `{"ChildPage": "/abs/.../child.tsx"}`
///   - `import { useData } from './hooks/useData'` with `./hooks/useData.ts`
///     → `{"useData": "/abs/.../hooks/useData.ts"}`
///   - `import { useData } from '@/hooks/useData'` with a tsconfig `paths`
///     alias → included, resolved through the project's resolver
///   - `import { useQuery } from '@tanstack/react-query'` → not included:
///     the resolver maps no npm package to a file
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

/// Local binding name → what that name was proven to stand for, for one file.
///
/// The map a JSX callee is stamped with at lowering ([`Expr::CompApp::origin`]).
/// `Arc` because every body of the file — every nested `FnLit`, every hook
/// body — shares the one map, and lowering clones its context freely.
///
/// [`Expr::CompApp::origin`]: crate::ir::expr::Expr::CompApp
#[derive(Debug, Clone, Default)]
pub struct JsxOrigins(Arc<HashMap<Symbol, Arc<ElementOrigin>>>);

impl JsxOrigins {
    /// What `local` was proven to name here, or `None` when this file
    /// neither declares nor imports it.
    pub fn get(&self, local: &str) -> Option<&Arc<ElementOrigin>> {
        self.0.get(local)
    }
}

/// Resolve every name `file` could write as a JSX callee to what it names:
/// its own top-level declarations and the imports the resolver maps to a
/// real file are program definitions; an import the resolver maps to no file
/// (a package, an unmapped alias, a missing file) is unseen code, which no
/// same-named component of the program stands for (#389).
///
/// Imports are laid down last on purpose — not because a file can both declare
/// and import one name (that is a redeclaration error), but so the rule is
/// stated once rather than depending on which pass ran first.
///
/// A name absent from the map is one the file neither declares nor imports
/// (a namespace member, a global, a binding of a body): the engine falls back
/// to resolution by name.
pub fn build_jsx_origins(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> JsxOrigins {
    let mut map: HashMap<Symbol, Arc<ElementOrigin>> = HashMap::new();
    for name in super::top_level_binding_names(program) {
        map.insert(
            name.clone(),
            Arc::new(ElementOrigin::Program(CompOrigin {
                file: current_file.to_path_buf(),
                name,
            })),
        );
    }
    for (local, resolved) in import_bindings(program, current_file, resolver) {
        let origin = match resolved {
            Some(r) => ElementOrigin::Program(CompOrigin {
                file: r.file,
                name: r.imported,
            }),
            None => ElementOrigin::Unseen,
        };
        map.insert(local, Arc::new(origin));
    }
    JsxOrigins(Arc::new(map))
}
