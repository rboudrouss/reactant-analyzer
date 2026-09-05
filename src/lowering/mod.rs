pub mod cfg_builder;
pub mod component_detector;
mod detector;
pub mod expr_lower;
pub mod hook_call_detect;
pub mod hook_detector;
pub mod hook_extractor;
pub mod import_resolution;
pub mod jsx_detect;
pub mod module_facts;
pub mod utility_detector;
pub mod utility_lowerer;

pub use crate::ir::{FileId, FileTable, SourceMap, compute_line_starts, offset_to_range};
pub use cfg_builder::{LowerCtx, build_cfg, build_fn_body_cfg};
pub use component_detector::{ComponentCandidate, detect_components};
pub use hook_detector::{HookCandidate, detect_custom_hooks};
pub use hook_extractor::{extract_handlers, extract_hooks, extract_subscriptions};
pub use import_resolution::{
    HookOrigin, JsxOrigins, ResolvedImport, build_hook_origins, build_jsx_origins,
    build_resolved_import_map, build_resolved_imports,
};
pub use module_facts::collect_module_facts;
pub use utility_detector::{UtilityCandidate, detect_utilities};
pub use utility_lowerer::{lower_utilities, lower_utilities_with_resolver};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, FormalParameters,
    FunctionBody, ImportDeclarationSpecifier, Program, Statement, VariableDeclarationKind,
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

/// Every name `program` binds at the top level, whatever the declaration form,
/// including the ones an `export` wraps.
///
/// Deliberately not "every component": the caller
/// ([`build_jsx_origins`]) is answering "does this file define the name
/// itself?", and a name a file declares can never *also* be one it imports.
/// A binding that turns out not to be a component simply matches nothing in
/// the registry.
pub(crate) fn top_level_binding_names(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    fn from_decl(decl: &Declaration, out: &mut Vec<String>) {
        match decl {
            Declaration::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    out.push(id.name.to_string());
                }
            }
            Declaration::ClassDeclaration(c) => {
                if let Some(id) = &c.id {
                    out.push(id.name.to_string());
                }
            }
            Declaration::VariableDeclaration(v) => {
                for d in &v.declarations {
                    if let Some(id) = d.id.get_binding_identifier() {
                        out.push(id.name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    for stmt in &program.body {
        match stmt {
            // `export default function Widget() {}` binds `Widget` here too,
            // and that is the name the component detector registers it under.
            // An anonymous default export binds nothing and is registered as
            // `DefaultExport`, which no JSX in this file can name.
            Statement::ExportDefaultDeclaration(e) => {
                if let ExportDefaultDeclarationKind::FunctionDeclaration(f) = &e.declaration
                    && let Some(id) = &f.id
                {
                    out.push(id.name.to_string());
                }
                if let ExportDefaultDeclarationKind::ClassDeclaration(c) = &e.declaration
                    && let Some(id) = &c.id
                {
                    out.push(id.name.to_string());
                }
            }
            Statement::ExportNamedDeclaration(e) => {
                if let Some(decl) = &e.declaration {
                    from_decl(decl, &mut out);
                }
            }
            _ => {
                if let Some(decl) = stmt.as_declaration() {
                    from_decl(decl, &mut out);
                }
            }
        }
    }
    out
}

/// A top-level function picked out by one of the detectors, ready for lowering:
/// its binding name, parameter list, and body. Shared by the component, hook,
/// and utility detectors — they differ in how they *classify* a function, not
/// in this output shape (all three feed `Candidate::build_cfg`).
#[derive(Debug)]
pub struct Candidate<'a> {
    pub name: String,
    pub params: &'a FormalParameters<'a>,
    pub body: &'a FunctionBody<'a>,
    /// `true` for a concise arrow (`x => expr`), whose body oxc stores as a
    /// single `ExpressionStatement` indistinguishable from `x => { expr; }`.
    /// The difference is the whole return value, so it has to travel with the
    /// body: lowered as a statement, `const useThing = () => useState(0)`
    /// returns unit, and every consumer of `useThing` is left with an
    /// unresolvable hook (#5).
    pub expression: bool,
}

impl Candidate<'_> {
    /// Lower this candidate's body to a CFG, honouring a concise arrow's
    /// implicit return.
    ///
    /// The single place that dispatch happens, so a new consumer of
    /// [`Candidate`] cannot forget it — which is how the flag came to be
    /// dropped in the first place.
    pub(crate) fn build_cfg(&self, ctx: &LowerCtx) -> (Vec<String>, crate::ir::cfg::CFG) {
        if self.expression {
            cfg_builder::build_expr_fn_body_cfg(self.params, self.body, ctx)
        } else {
            cfg_builder::build_fn_body_cfg(self.params, self.body, ctx)
        }
    }
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

/// Module-level bindings of `program` that are proven React contexts.
///
/// The cross-file pass in [`crate::resolver::lower_files_with`] needs this for
/// *every* file, including files with no component of their own — a
/// `contexts/ctx.ts` that only exports the context is the common shape.
pub(crate) fn scan_context_names(program: &Program, file: &Path) -> HashSet<String> {
    let react_ns = build_react_ns(program);
    collect_module_consts(program, &react_ns, file)
        .into_iter()
        .filter(|(_, init)| matches!(init, ModuleConstInit::Context(_)))
        .map(|(name, _)| name)
        .collect()
}

/// Local names bound to React's `createContext`, including an aliased import
/// (`import { createContext as ctx } from "react"`). Namespace forms
/// (`React.createContext`) go through [`build_react_ns`] instead.
fn react_create_context_names(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
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
            if let ImportDeclarationSpecifier::ImportSpecifier(s) = spec
                && s.imported.name() == "createContext"
            {
                names.insert(s.local.name.to_string());
            }
        }
    }
    names
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
fn collect_module_consts(
    program: &Program,
    react_ns: &HashSet<String>,
    file: &Path,
) -> HashMap<Var, ModuleConstInit> {
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

    // `createContext(…)` reached through a React binding: a bare name imported
    // from "react", or `<ns>.createContext` where `ns` is the react module
    // (`React` is accepted unimported, matching `ImportCtx::callee_is_react`).
    let create_context = react_create_context_names(program);
    let is_create_context = |call: &oxc_ast::ast::CallExpression| match &call.callee {
        Expression::Identifier(id) => create_context.contains(id.name.as_str()),
        Expression::StaticMemberExpression(m) => {
            m.property.name == "createContext"
                && match &m.object {
                    Expression::Identifier(ns) => {
                        react_ns.contains(ns.name.as_str()) || ns.name == "React"
                    }
                    _ => false,
                }
        }
        _ => false,
    };

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
            } else if let Expression::CallExpression(call) = init
                && is_create_context(call)
            {
                // A locally-created context IS its own origin (#109).
                map.insert(
                    id.name.to_string(),
                    ModuleConstInit::Context(crate::ir::ContextId {
                        origin_file: file.to_path_buf(),
                        origin_name: id.name.to_string(),
                    }),
                );
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
    lower_custom_hooks_with_resolver(
        program,
        source,
        file,
        files,
        &DefaultImportResolver::default(),
    )
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
    // One context per file: it carries the shared allocation-site counter
    // (`ExprIds`, #134) and the JSX callee origins (#7) into every body.
    let ctx = LowerCtx::new(
        SourceMap::new(source, files.intern(file)),
        build_jsx_origins(program, file, resolver),
    );
    let origins = build_hook_origins(program, file, resolver);
    let react_ns = build_react_ns(program);
    let candidates = detect_custom_hooks(program);
    let local_hooks: HashSet<String> = candidates.iter().map(|c| c.name.clone()).collect();
    let imports = hook_extractor::ImportCtx {
        origins: &origins,
        react_ns: &react_ns,
        local_hooks: &local_hooks,
        current_file: Some(file),
    };
    candidates
        .into_iter()
        .map(|candidate| {
            let (params, mut body_cfg) = candidate.build_cfg(&ctx);
            let (mut hooks, hook_provenance, mut next_label) =
                extract_hooks(&mut body_cfg, &imports);
            extract_handlers(&body_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            HookIR {
                file: file.to_path_buf(),
                name: candidate.name,
                params,
                body_cfg,
                hooks,
                hook_provenance,
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
    lower_program_with_resolver(
        program,
        source,
        file,
        files,
        &DefaultImportResolver::default(),
    )
}

/// Plugin-friendly variant of [`lower_program`].
pub fn lower_program_with_resolver(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
    resolver: &dyn ImportResolver,
) -> Vec<ComponentIR> {
    // One context per file: it carries the shared allocation-site counter
    // (`ExprIds`, #134) and the JSX callee origins (#7) into every body.
    let ctx = LowerCtx::new(
        SourceMap::new(source, files.intern(file)),
        build_jsx_origins(program, file, resolver),
    );
    let origins = build_hook_origins(program, file, resolver);
    let react_ns = build_react_ns(program);
    let module_consts = Arc::new(collect_module_consts(program, &react_ns, file));
    let local_hooks: HashSet<String> = detect_custom_hooks(program)
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let imports = hook_extractor::ImportCtx {
        origins: &origins,
        react_ns: &react_ns,
        local_hooks: &local_hooks,
        current_file: Some(file),
    };
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let (param_names, mut render_cfg) = candidate.build_cfg(&ctx);
            let (mut hooks, hook_provenance, mut next_label) =
                extract_hooks(&mut render_cfg, &imports);
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
                hook_provenance,
                module_consts: module_consts.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn consts(src: &str) -> HashMap<Var, ModuleConstInit> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(
            ret.diagnostics.is_empty(),
            "parse errors: {:?}",
            ret.diagnostics
        );
        let react_ns = build_react_ns(&ret.program);
        collect_module_consts(&ret.program, &react_ns, Path::new("t.tsx"))
    }

    fn is_context(map: &HashMap<Var, ModuleConstInit>, name: &str) -> bool {
        matches!(map.get(name), Some(ModuleConstInit::Context(_)))
    }

    #[test]
    fn react_create_context_is_a_context_const() {
        let m =
            consts("import { createContext } from \"react\";\nconst Ctx = createContext(null);");
        assert!(is_context(&m, "Ctx"));
    }

    #[test]
    fn namespace_and_aliased_forms_are_contexts() {
        let m = consts(
            "import * as R from \"react\";\nimport { createContext as mk } from \"react\";\n\
             const A = R.createContext(0);\nconst B = React.createContext(0);\nconst C = mk(0);",
        );
        for name in ["A", "B", "C"] {
            assert!(is_context(&m, name), "{name} should be a context");
        }
    }

    /// The exception the `Context` arm carves out is narrow on purpose: an
    /// opaque call still has an unknown kind and stays out of the map, or a
    /// wide primitive slot would read as per-render motion.
    #[test]
    fn other_calls_are_still_skipped() {
        let m = consts(
            "import { createContext } from \"other\";\n\
             const A = makeThing();\nconst B = createContext(null);\nconst C = obj.createContext(0);",
        );
        assert!(!m.contains_key("A"), "opaque call must stay out");
        assert!(!m.contains_key("B"), "createContext from another package");
        assert!(
            !m.contains_key("C"),
            "createContext on a non-react receiver"
        );
    }

    /// A context const seeds as a Stable reference, so it is also a precision
    /// win independent of the provider relation.
    #[test]
    fn context_and_literal_consts_coexist() {
        let m = consts(
            "import { createContext } from \"react\";\n\
             const Ctx = createContext(null);\nconst N = 3;\nconst O = { a: 1 };",
        );
        assert!(is_context(&m, "Ctx"));
        assert!(matches!(m.get("N"), Some(ModuleConstInit::Prim(_))));
        assert!(matches!(m.get("O"), Some(ModuleConstInit::Ref)));
    }
}
