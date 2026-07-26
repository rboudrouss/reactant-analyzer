pub mod cfg_builder;
pub mod component_detector;
mod detector;
pub mod expr_lower;
pub mod hook_detector;
pub mod hook_extractor;
pub mod import_resolution;
pub mod jsx_detect;
pub mod utility_detector;
pub mod utility_lowerer;

pub use crate::ir::{FileId, FileTable, SourceMap, compute_line_starts, offset_to_range};
pub use cfg_builder::{build_cfg, build_fn_body_cfg};
pub use component_detector::{ComponentCandidate, detect_components};
pub use hook_detector::{HookCandidate, detect_custom_hooks};
pub use hook_extractor::{extract_handlers, extract_hooks, extract_subscriptions};
pub use import_resolution::build_resolved_import_map;
pub use utility_detector::{UtilityCandidate, detect_utilities};
pub use utility_lowerer::{lower_utilities, lower_utilities_with_resolver};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc_ast::ast::{
    BindingPattern, Declaration, Expression, FormalParameters, FunctionBody,
    ImportDeclarationSpecifier, Program, Statement, VariableDeclarationKind,
};

use crate::{
    ir::{
        component::{ComponentIR, ModuleConstInit},
        expr::Prim,
        hook_ir::HookIR,
        types::Var,
    },
    resolver::{DefaultImportResolver, ImportResolver},
};

/// React's naming rule for a custom hook: `use` followed by an uppercase letter
/// or a digit (`useCounter`, `use2FA`). A lowercase 4th char (`useful`,
/// `userId`) is NOT a hook — such a function cannot legally call hooks (Rules of
/// Hooks), so it is a plain utility.
///
/// Single source of truth for the hook/utility classification boundary: the
/// hook detector and the utility detector used to hard-code divergent rules
/// (`starts_with("use") && len > 3` vs this one), so `useful` was classified as
/// BOTH a hook and a utility. Both now call this.
pub(crate) fn is_hook_name(name: &str) -> bool {
    name.starts_with("use")
        && name
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// A top-level function picked out by one of the detectors, ready for lowering:
/// its binding name, parameter list, and body. Shared by the component, hook,
/// and utility detectors — they differ in how they *classify* a function, not
/// in this output shape (all three feed `build_fn_body_cfg(params, body)`).
#[derive(Debug)]
pub struct Candidate<'a> {
    pub name: String,
    pub params: &'a FormalParameters<'a>,
    pub body: &'a FunctionBody<'a>,
}

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

/// Local names bound to the `react` module itself: `import React from
/// "react"` and `import * as R from "react"`. `R.useMemo(...)` is React's
/// hook only through one of these bindings (see `ImportCtx::callee_is_react`).
fn build_react_ns(program: &Program) -> HashSet<String> {
    let mut ns = HashSet::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        if decl.source.value.as_str() != "react" {
            continue;
        }
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        for spec in specifiers {
            match spec {
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    ns.insert(s.local.name.to_string());
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    ns.insert(s.local.name.to_string());
                }
                ImportDeclarationSpecifier::ImportSpecifier(_) => {}
            }
        }
    }
    ns
}

/// Collect module-level `const` bindings whose initializer kind is
/// syntactically certain (see [`ModuleConstInit`]).
///
/// JS semantics: a module-level `const` is evaluated once when the module
/// loads, so its identity never changes across renders. Function-valued
/// initializers (arrow / function expressions) are skipped entirely: those
/// are components, custom hooks, or utilities, each with dedicated handling
/// (detection, inlining), and seeding them here would shadow that machinery.
/// Opaque initializers (calls, imported values, conditionals) are skipped
/// too: their kind is unknown and the value domain has no sound encoding
/// for "unknown kind, constant across renders" — they stay ⊤.
fn collect_module_consts(program: &Program) -> HashMap<Var, ModuleConstInit> {
    fn peel_ts<'e, 'a>(mut expr: &'e Expression<'a>) -> &'e Expression<'a> {
        loop {
            expr = match expr {
                Expression::TSAsExpression(e) => &e.expression,
                Expression::TSSatisfiesExpression(e) => &e.expression,
                Expression::TSNonNullExpression(e) => &e.expression,
                Expression::TSTypeAssertion(e) => &e.expression,
                Expression::ParenthesizedExpression(e) => &e.expression,
                _ => return expr,
            };
        }
    }

    fn lit_prim(expr: &Expression) -> Option<Prim> {
        match expr {
            Expression::BooleanLiteral(b) => Some(Prim::Bool(b.value)),
            Expression::NullLiteral(_) => Some(Prim::Null),
            Expression::NumericLiteral(n) => {
                if n.value.fract() == 0.0 && n.value.abs() < i32::MAX as f64 {
                    Some(Prim::Int(n.value as i32))
                } else {
                    Some(Prim::Float(n.value))
                }
            }
            Expression::StringLiteral(s) => Some(Prim::String(s.value.to_string())),
            _ => None,
        }
    }

    let mut map = HashMap::new();
    for stmt in &program.body {
        let decl = match stmt {
            Statement::VariableDeclaration(d) => d,
            Statement::ExportNamedDeclaration(exp) => match &exp.declaration {
                Some(Declaration::VariableDeclaration(d)) => d,
                _ => continue,
            },
            _ => continue,
        };
        if decl.kind != VariableDeclarationKind::Const {
            continue;
        }
        for vd in &decl.declarations {
            let BindingPattern::BindingIdentifier(id) = &vd.id else {
                continue;
            };
            let Some(init) = &vd.init else { continue };
            let init = peel_ts(init);
            if let Some(p) = lit_prim(init) {
                map.insert(id.name.to_string(), ModuleConstInit::Prim(p));
            } else if matches!(
                init,
                Expression::ObjectExpression(_)
                    | Expression::ArrayExpression(_)
                    | Expression::NewExpression(_)
                    | Expression::RegExpLiteral(_)
                    | Expression::JSXElement(_)
                    | Expression::JSXFragment(_)
            ) {
                map.insert(id.name.to_string(), ModuleConstInit::Ref);
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
///
/// `source` is the file's text (for the span line table); `files` interns
/// `file` so every produced span carries its [`FileId`] (ADR-019).
pub fn lower_custom_hooks(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
) -> Vec<HookIR> {
    lower_custom_hooks_with_resolver(program, source, file, files, &DefaultImportResolver::default())
}

/// Plugin-friendly variant of [`lower_custom_hooks`] that accepts a custom
/// `ImportResolver`.
pub fn lower_custom_hooks_with_resolver(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
    resolver: &dyn ImportResolver,
) -> Vec<HookIR> {
    let smap = SourceMap::new(source, files.intern(file));
    let import_map = build_import_map(program);
    let resolved_import_map: HashMap<String, PathBuf> =
        build_resolved_import_map(program, file, resolver);
    let react_ns = build_react_ns(program);
    let candidates = detect_custom_hooks(program);
    let local_hooks: HashSet<String> = candidates.iter().map(|c| c.name.clone()).collect();
    let imports = hook_extractor::ImportCtx {
        import_map: &import_map,
        resolved_import_map: &resolved_import_map,
        react_ns: &react_ns,
        local_hooks: &local_hooks,
    };
    candidates
        .into_iter()
        .map(|candidate| {
            let (params, mut body_cfg) = build_fn_body_cfg(candidate.params, candidate.body, &smap);
            let (mut hooks, mut next_label) = extract_hooks(&mut body_cfg, &imports);
            extract_handlers(&body_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            HookIR {
                file: file.to_path_buf(),
                name: candidate.name,
                params,
                body_cfg,
                hooks,
            }
        })
        .collect()
}

/// Stage 3 entry point: lower all React components in `program` to `ComponentIR`.
///
/// `file` is the absolute path of the source file. Uses
/// [`DefaultImportResolver`]; see [`lower_program_with_resolver`] for the
/// plugin variant.
pub fn lower_program(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
) -> Vec<ComponentIR> {
    lower_program_with_resolver(program, source, file, files, &DefaultImportResolver::default())
}

/// Plugin-friendly variant of [`lower_program`].
pub fn lower_program_with_resolver(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
    resolver: &dyn ImportResolver,
) -> Vec<ComponentIR> {
    let smap = SourceMap::new(source, files.intern(file));
    let import_map = build_import_map(program);
    let resolved_import_map: HashMap<String, PathBuf> =
        build_resolved_import_map(program, file, resolver);
    let module_consts = Arc::new(collect_module_consts(program));
    let react_ns = build_react_ns(program);
    let local_hooks: HashSet<String> = detect_custom_hooks(program)
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let imports = hook_extractor::ImportCtx {
        import_map: &import_map,
        resolved_import_map: &resolved_import_map,
        react_ns: &react_ns,
        local_hooks: &local_hooks,
    };
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let (param_names, mut render_cfg) =
                build_fn_body_cfg(candidate.params, candidate.body, &smap);
            let (mut hooks, mut next_label) = extract_hooks(&mut render_cfg, &imports);
            extract_handlers(&render_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            let param = param_names
                .into_iter()
                .next()
                .unwrap_or_else(|| "props".to_string());
            let dom_props = Arc::new(component_detector::collect_dom_props(
                candidate.params,
                program,
            ));
            ComponentIR {
                file: file.to_path_buf(),
                name: candidate.name,
                param,
                dom_props,
                render_cfg,
                hooks,
                module_consts: module_consts.clone(),
            }
        })
        .collect()
}
