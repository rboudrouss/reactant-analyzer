mod body_scan;
pub mod cfg_builder;
pub mod class_lowerer;
pub mod component_detector;
mod detector;
pub mod expr_lower;
pub mod hook_detector;
pub mod hook_extractor;
pub mod import_resolution;
mod jsx_scope;
pub mod module_facts;
pub mod react_wrappers;
pub mod utility_detector;
pub mod utility_lowerer;

pub use crate::ir::{FileId, FileTable, SourceMap, compute_line_starts, offset_to_range};
pub use cfg_builder::{LowerCtx, build_cfg, build_fn_body_cfg};
pub use class_lowerer::lower_classes_with_resolver;
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
        component::{ComponentIR, ModuleConstInit, RefPassing},
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
    /// What a call of the function evaluates to: an `async` or generator
    /// function's call is not its `return` value (#317).
    pub kind: cfg_builder::FnKind,
    /// An arrow function, which binds no `arguments` of its own (#324).
    pub arrow: bool,
    /// The React wrapper the function is the argument of (`memo(f)`,
    /// `forwardRef(f)`, #64): React renders it as a component, and through a
    /// `forwardRef` calls it with the element's `ref` second.
    pub wrapped: Option<react_wrappers::Wrapper>,
}

impl Candidate<'_> {
    /// Lower this candidate's body to a CFG, honouring a concise arrow's
    /// implicit return and what a call of the function's kind evaluates to.
    ///
    /// The single place that dispatch happens, so a new consumer of
    /// [`Candidate`] cannot forget it — which is how the flag came to be
    /// dropped in the first place.
    pub(crate) fn build_cfg(&self, ctx: &LowerCtx) -> (Vec<String>, crate::ir::cfg::CFG) {
        self.build_cfg_as(self.kind, ctx)
    }

    /// [`Self::build_cfg`] for a component's render: React awaits an async
    /// component's promise and renders what the body returns, so the render's
    /// `return` is its output whatever the function's kind.
    pub(crate) fn build_render_cfg(&self, ctx: &LowerCtx) -> (Vec<String>, crate::ir::cfg::CFG) {
        self.build_cfg_as(cfg_builder::FnKind::Plain, ctx)
    }

    fn build_cfg_as(
        &self,
        kind: cfg_builder::FnKind,
        ctx: &LowerCtx,
    ) -> (Vec<String>, crate::ir::cfg::CFG) {
        let forward_ref = self.wrapped == Some(react_wrappers::Wrapper::ForwardRef);
        cfg_builder::build_candidate_cfg(
            self.params,
            self.body,
            cfg_builder::BodyShape {
                expression: self.expression,
                arrow: self.arrow,
                kind,
                forward_ref,
            },
            ctx,
        )
    }

    /// The default of each parameter, aligned with [`Self::build_cfg`]'s
    /// names.
    pub(crate) fn param_defaults(&self, ctx: &LowerCtx) -> Vec<Option<crate::ir::expr::Expr>> {
        cfg_builder::param_defaults(self.params, ctx)
    }
}

/// Local names bound to the `react` module itself: `import React from
/// "react"` and `import * as R from "react"`. `R.useMemo(...)` is React's
/// hook only through one of these bindings (see `ImportCtx::classify_callee`).
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

/// The module-level `const`s of `program` whose kind is known
/// ([`ModuleConstInit`]), its proven React contexts among them.
///
/// The cross-file pass in [`crate::resolver::lower_files_with`] needs this for
/// *every* file, including files with no component of their own: a
/// `contexts/ctx.ts` that only exports the context, or the file of a hook
/// another file's component inlines, whose body reads its own file's
/// `const`s (#354).
pub(crate) fn module_consts(program: &Program, file: &Path) -> HashMap<Var, ModuleConstInit> {
    collect_module_consts(program, &build_react_ns(program), file)
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
    // (`React` is accepted unimported, matching `ImportCtx::classify_callee`).
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
    let (mut hooks, names, values) = lower_file_hooks(program, source, file, files, resolver);
    if !values.is_empty() {
        let functions = lower_utilities_with_resolver(program, source, file, files, resolver);
        hooks.extend(lower_hook_values(
            &values,
            &functions,
            &HashMap::from([(file.to_path_buf(), names)]),
            &[(
                file.to_path_buf(),
                build_resolved_imports(program, file, resolver),
            )],
            &crate::ir::module::ReExportGraph::default(),
        ));
    }
    hooks
}

/// The names a file's hook extraction reads — its hook imports, the names
/// it binds to the `react` module, its own hooks — kept past the file's
/// lowering, so that a closure of this file that a hook value of another
/// file runs is read as this file reads it (#344).
#[derive(Debug)]
pub struct HookNames {
    file: std::path::PathBuf,
    origins: HashMap<String, HookOrigin>,
    react_ns: HashSet<String>,
    local_hooks: HashSet<String>,
}

impl HookNames {
    fn imports(&self) -> hook_extractor::ImportCtx<'_> {
        hook_extractor::ImportCtx {
            origins: &self.origins,
            react_ns: &self.react_ns,
            local_hooks: &self.local_hooks,
            current_file: Some(&self.file),
            bound: hook_extractor::ImportCtx::unbound(),
        }
    }
}

/// A hook a module `const` binds to what a call returns at module time
/// (`const useX = make(a)`, #342), whose body is found once every file is
/// lowered ([`lower_hook_values`]): the function called may be another
/// file's (#344).
#[derive(Debug)]
pub struct HookValueSite {
    /// The file the binding is in.
    file: std::path::PathBuf,
    name: String,
    /// The function called: the file's own, or where its import resolves.
    callee: ResolvedImport,
    /// What it is called with: as [`cfg_builder::module_args`] keeps the
    /// arguments when the callee is the file's own; else a name is the
    /// primitive the file's `const` binds it to, and ⊤ when it binds none — a
    /// name of this file is not one of the callee's.
    call: ModuleCall,
}

/// A call of a program function made once, at module time, as a body of the
/// function's file reads it (#342, #383).
#[derive(Debug, Clone)]
struct ModuleCall {
    /// The arguments: a primitive, a name of the function's module or a
    /// member chain off one, as written, and ⊤ for anything else.
    args: Vec<crate::ir::Expr>,
    /// What a call of each argument is: `Some(Some(hook))` the hook it names,
    /// `Some(None)` no hook (a plain value), `None` unknown (an argument that
    /// is ⊤): the parameter is then bound to nothing.
    hooks: Vec<Option<Option<hook_extractor::ResolvedHookCall>>>,
}

/// The custom hooks `program` defines, lowered, with the names its hook
/// extraction read and the hook values it binds, which
/// [`lower_hook_values`] lowers once the functions they call are.
pub(crate) fn lower_file_hooks(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
    resolver: &dyn ImportResolver,
) -> (Vec<HookIR>, HookNames, Vec<HookValueSite>) {
    // One context per file: it carries the shared allocation-site counter
    // (`ExprIds`, #134) and the JSX callee origins (#7) into every body.
    let ctx = LowerCtx::new(
        SourceMap::new(source, files.intern(file)),
        build_jsx_origins(program, file, resolver),
    );
    let origins = build_hook_origins(program, file, resolver);
    let react_ns = build_react_ns(program);
    let candidates = detect_custom_hooks(program);
    let (local_hooks, aliases) = local_hooks(program, &origins, &react_ns, file, resolver);
    let names = HookNames {
        file: file.to_path_buf(),
        origins,
        react_ns,
        local_hooks,
    };
    let imports = names.imports();
    let mut out: Vec<HookIR> = candidates
        .into_iter()
        .map(|candidate| {
            let (params, mut body_cfg) = candidate.build_cfg(&ctx);
            let param_defaults = candidate.param_defaults(&ctx);
            let (mut hooks, hook_provenance, mut next_label) =
                extract_hooks(&mut body_cfg, &imports, &ctx.ids);
            extract_handlers(&body_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            HookIR {
                file: file.to_path_buf(),
                name: candidate.name,
                params,
                param_defaults,
                body_cfg,
                hooks,
                hook_provenance,
                alias_of: None,
                body_file: None,
            }
        })
        .collect();
    out.extend(aliases.into_iter().map(|(alias, hook)| HookIR {
        file: file.to_path_buf(),
        name: alias.name,
        params: vec![],
        param_defaults: vec![],
        body_cfg: crate::ir::cfg::CFG {
            entry: 0,
            blocks: std::collections::BTreeMap::from([(
                0,
                crate::ir::cfg::BasicBlock {
                    id: 0,
                    stmts: vec![],
                    term: crate::ir::cfg::Terminator::Return(
                        crate::ir::expr::Expr::Lit(crate::ir::expr::Prim::Unit),
                        None,
                    ),
                },
            )]),
            edges: vec![],
        },
        hooks: vec![],
        hook_provenance: vec![],
        alias_of: Some(hook),
        body_file: None,
    }));
    let values = hook_values(program, file, resolver);
    let consts = match values.is_empty() {
        true => HashMap::new(),
        false => collect_module_consts(program, &names.react_ns, file),
    };
    let values = values
        .into_iter()
        .filter_map(|(value, callee)| hook_value_site(value, callee, &ctx, &imports, &consts, file))
        .collect();
    ctx.name_temps(source, files);
    (out, names, values)
}

/// The hooks `program` binds to the value a call returns at module time
/// (#342), each with the function called: one of the file's own, or one an
/// import resolves to a file of the program, under the name that file
/// exports it as (#344).
fn hook_values<'a>(
    program: &'a Program<'a>,
    file: &Path,
    resolver: &dyn ImportResolver,
) -> Vec<(hook_detector::HookValue<'a>, ResolvedImport)> {
    let values = hook_detector::detect_hook_values(program);
    if values.is_empty() {
        return Vec::new();
    }
    let functions: HashSet<String> = detect_utilities(program)
        .into_iter()
        .map(|f| f.name)
        .collect();
    let imports = build_resolved_imports(program, file, resolver);
    values
        .into_iter()
        .filter_map(|value| {
            let callee = match functions.contains(value.callee) {
                true => ResolvedImport {
                    file: file.to_path_buf(),
                    imported: value.callee.to_string(),
                },
                false => imports.get(value.callee)?.clone(),
            };
            Some((value, callee))
        })
        .collect()
}

/// The call `value` makes, as [`lower_hook_values`] reads it once `callee`
/// is found. `None` when a spread hides which parameter takes what.
fn hook_value_site(
    value: hook_detector::HookValue,
    callee: ResolvedImport,
    ctx: &LowerCtx,
    imports: &hook_extractor::ImportCtx,
    consts: &HashMap<Var, ModuleConstInit>,
    file: &Path,
) -> Option<HookValueSite> {
    use crate::ir::{Expr, expr::SummaryValue};
    let args = cfg_builder::module_args(value.args, ctx)?;
    let hooks = args
        .iter()
        .map(|arg| match arg.peel_ts() {
            e @ (Expr::Var(_) | Expr::FieldAccess { .. }) => Some(imports.classify_callee(e)),
            Expr::Lit(_) => Some(None),
            _ => None,
        })
        .collect();
    let own = callee.file == file;
    let args = args
        .into_iter()
        .map(|arg| match arg.peel_ts() {
            _ if own => arg,
            Expr::Var(v) => match consts.get(v) {
                Some(ModuleConstInit::Prim(p)) => Expr::Lit(p.clone()),
                _ => Expr::SummaryVal(SummaryValue::Top),
            },
            Expr::FieldAccess { .. } => Expr::SummaryVal(SummaryValue::Top),
            _ => arg,
        })
        .collect();
    Some(HookValueSite {
        file: file.to_path_buf(),
        name: value.name,
        callee,
        call: ModuleCall { args, hooks },
    })
}

/// The hooks `values` bind, each what the function it calls returns
/// (`lower_hook_value`): the function is one of `functions`, found where
/// the call names it — the binding's file, or the file an import resolves
/// to, through its re-exports — and its closure is read with the names of
/// its own file (`names`). Each file's resolved `imports` find a function a
/// factory's `return` names (#383). A value whose function is none of them
/// builds no hook: it stays one the analysis does not see.
pub fn lower_hook_values(
    values: &[HookValueSite],
    functions: &[crate::ir::FunctionIR],
    names: &HashMap<std::path::PathBuf, HookNames>,
    imports: &[(std::path::PathBuf, HashMap<String, ResolvedImport>)],
    re_exports: &crate::ir::module::ReExportGraph,
) -> Vec<HookIR> {
    if values.is_empty() {
        return Vec::new();
    }
    let program = Functions {
        defined: functions
            .iter()
            .map(|f| ((f.file.as_path(), f.name.as_str()), f))
            .collect(),
        imports: imports.iter().map(|(f, m)| (f.as_path(), m)).collect(),
        re_exports,
        names,
    };
    values
        .iter()
        .filter_map(|value| {
            let factory = program.origin(&value.callee.file, &value.callee.imported)?;
            lower_hook_value(value, factory, &program)
        })
        .collect()
}

/// The program's functions a hook value's body is found among (#344, #383).
struct Functions<'a> {
    /// The lowered functions, by file and name.
    defined: HashMap<(&'a Path, &'a str), &'a crate::ir::FunctionIR>,
    /// Each file's imports, resolved.
    imports: HashMap<&'a Path, &'a HashMap<String, ResolvedImport>>,
    re_exports: &'a crate::ir::module::ReExportGraph,
    /// What each file's hook extraction reads.
    names: &'a HashMap<std::path::PathBuf, HookNames>,
}

impl<'a> Functions<'a> {
    /// The function `file` exports as `name`, through its re-exports.
    fn origin(&self, file: &Path, name: &str) -> Option<&'a crate::ir::FunctionIR> {
        let defines = |f: &Path, n: &str| self.defined.contains_key(&(f, n));
        let (file, name) = self.re_exports.origin(file, name, &defines)?;
        self.defined.get(&(file.as_path(), name.as_str())).copied()
    }

    /// The function a name of `file`'s module scope is: one `file` writes,
    /// or one an import of `file` resolves to.
    fn named(&self, file: &Path, name: &str) -> Option<&'a crate::ir::FunctionIR> {
        match self.imports.get(file).and_then(|m| m.get(name)) {
            Some(import) => self.origin(&import.file, &import.imported),
            None => self.defined.get(&(file, name)).copied(),
        }
    }
}

/// The hook `value` binds: what `factory` returns ([`returned_closures`]),
/// run as a custom hook's body — one function, or one of several chosen once
/// per program run ([`choice_of`], #344). Its own hooks are those its file
/// names. `None`, the hook staying one the analysis does not see, unless
/// every return of `factory` is a function the analysis reads, all written in
/// one file: a body reads one file's names.
fn lower_hook_value(
    value: &HookValueSite,
    factory: &crate::ir::FunctionIR,
    program: &Functions,
) -> Option<HookIR> {
    let candidates = returned_closures(program, factory, &value.call, 0)?;
    let file = &candidates.first()?.from.file;
    if candidates.iter().any(|c| &c.from.file != file) {
        return None;
    }
    let names = program.names.get(file)?;
    let written: Vec<&crate::ir::FunctionIR> = std::iter::once(factory)
        .chain(candidates.iter().map(|c| c.from))
        .collect();
    let (closure_params, mut body_cfg, bound) = choice_of(candidates)?;
    let ids = cfg_builder::ExprIds::above(
        written
            .iter()
            .map(|f| &f.body_cfg)
            .chain(std::iter::once(&body_cfg)),
    );
    let imports = hook_extractor::ImportCtx {
        bound: &bound,
        ..names.imports()
    };
    let (mut hooks, hook_provenance, mut next_label) = extract_hooks(&mut body_cfg, &imports, &ids);
    extract_handlers(&body_cfg, &mut hooks, &mut next_label);
    extract_subscriptions(&mut hooks, &mut next_label);
    Some(HookIR {
        file: value.file.clone(),
        name: value.name.clone(),
        param_defaults: vec![None; closure_params.len()],
        params: closure_params,
        body_cfg,
        hooks,
        hook_provenance,
        alias_of: None,
        body_file: (file != &value.file).then(|| file.clone()),
    })
}

/// A function a hook value may be (#342, #344, #383).
struct ValueBody<'a> {
    params: Vec<Var>,
    body: crate::ir::cfg::CFG,
    /// Binds what the body reads of the function that returned it.
    prelude: Vec<crate::ir::Stmt>,
    /// What a call of each name `prelude` binds is.
    bound: HashMap<Var, Option<hook_extractor::ResolvedHookCall>>,
    /// The function it is written in.
    from: &'a crate::ir::FunctionIR,
}

impl<'a> ValueBody<'a> {
    /// A function of the program handed back as a value: its own body.
    fn function(f: &'a crate::ir::FunctionIR) -> Self {
        ValueBody {
            params: f.params.clone(),
            body: f.body_cfg.clone(),
            prelude: Vec::new(),
            bound: HashMap::new(),
            from: f,
        }
    }

    /// The closure (`params`, `body`) `f`, called with `call`, returns. Each
    /// parameter of `f` it reads holds the call's argument — ⊤ when `f` or a
    /// body nested in it reassigns it (`reassigned`), a default included —
    /// and a call of it is a call of the hook the argument names; each other
    /// local of `f` it reads is ⊤, since it lives as long as the module and
    /// any call may move it (#342).
    fn closure(
        f: &'a crate::ir::FunctionIR,
        call: &ModuleCall,
        reassigned: &HashSet<&str>,
        params: &[Var],
        body: &crate::ir::cfg::CFG,
    ) -> Self {
        use crate::ir::{Expr, Stmt, expr::SummaryValue, free_vars::compute_free_vars};
        let reads: HashSet<Var> = compute_free_vars(body)
            .into_iter()
            .filter(|v| !params.contains(v))
            .collect();
        let mut prelude = Vec::new();
        let mut bound = HashMap::new();
        for (i, param) in f.params.iter().enumerate() {
            if !reads.contains(param) {
                continue;
            }
            let reassigned = reassigned.contains(param.as_str());
            let rhs = match call.args.get(i) {
                _ if reassigned => Expr::SummaryVal(SummaryValue::Top),
                Some(arg) => arg.clone(),
                None => Expr::Lit(Prim::Unit),
            };
            let hook = match call.hooks.get(i) {
                _ if reassigned => None,
                Some(Some(hook)) => Some(hook.clone()),
                Some(None) => None,
                None => Some(None),
            };
            if let Some(hook) = hook {
                bound.insert(param.clone(), hook);
            }
            prelude.push(Stmt::Let {
                var: param.clone(),
                rhs,
                span: None,
            });
        }
        let locals = crate::ir::bound_vars(&f.body_cfg, &[]);
        let captured: std::collections::BTreeSet<&Var> = reads
            .iter()
            .filter(|v| locals.contains(*v) && !f.params.contains(v))
            .collect();
        prelude.extend(captured.into_iter().map(|var| Stmt::Let {
            var: var.clone(),
            rhs: Expr::SummaryVal(SummaryValue::Top),
            span: None,
        }));
        ValueBody {
            params: params.to_vec(),
            body: body.clone(),
            prelude,
            bound,
            from: f,
        }
    }
}

/// The call of `g` with `args` that `f`, itself called with `call`, makes, as
/// `g`'s file reads it (#383): a parameter of `f` is what `call` hands it (⊤
/// when `f` reassigns it), a primitive and a name of `f`'s module are as
/// written, anything else is ⊤ — a local of `f` included, as a closure reads
/// it — and a name is ⊤ in another file, whose names are not `f`'s. `None`
/// when a spread hides which parameter takes what.
fn call_of(
    program: &Functions,
    f: &crate::ir::FunctionIR,
    call: &ModuleCall,
    local: &dyn Fn(&str) -> bool,
    reassigned: &HashSet<&str>,
    args: &[crate::ir::Expr],
    g: &crate::ir::FunctionIR,
) -> Option<ModuleCall> {
    use crate::ir::{Expr, expr::SummaryValue};
    let imports = program.names.get(&f.file)?.imports();
    let top = || Expr::SummaryVal(SummaryValue::Top);
    let (args, hooks) = args
        .iter()
        .map(|arg| {
            let (arg, hook) = match arg.peel_ts() {
                Expr::Spread(_) => return None,
                lit @ Expr::Lit(_) => (lit.clone(), Some(None)),
                Expr::Var(v) => match f.params.iter().position(|p| p == v) {
                    Some(_) if reassigned.contains(v.as_str()) => (top(), None),
                    Some(i) => (
                        call.args.get(i).cloned().unwrap_or(Expr::Lit(Prim::Unit)),
                        call.hooks.get(i).cloned().unwrap_or(Some(None)),
                    ),
                    None if local(v) => (top(), None),
                    None => (
                        arg.peel_ts().clone(),
                        Some(imports.classify_callee(arg.peel_ts())),
                    ),
                },
                _ => (top(), None),
            };
            let named = matches!(arg.peel_ts(), Expr::Var(_) | Expr::FieldAccess { .. });
            Some(match named && g.file != f.file {
                true => (top(), hook),
                false => (arg, hook),
            })
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .unzip();
    Some(ModuleCall { args, hooks })
}

/// Every name a body nested in `cfg` assigns, and `cfg` itself when `own`.
fn assigned_below(cfg: &crate::ir::cfg::CFG, own: bool) -> HashSet<&str> {
    own.then_some(cfg)
        .into_iter()
        .chain(
            crate::ir::bindings::nested_bodies(cfg)
                .into_iter()
                .map(|(_, c)| c),
        )
        .flat_map(|c| c.blocks.values())
        .flat_map(|b| &b.stmts)
        .filter_map(|s| match s {
            crate::ir::Stmt::Assign { var, .. } => Some(var.as_str()),
            _ => None,
        })
        .collect()
}

/// How many calls deep a returned call is followed (#383): zustand's `create`
/// needs one; a factory that calls itself reaches the bound and fails closed.
const MAX_CALLS: usize = 3;

/// Every function `f`, called with `call`, may return, in source order. A
/// reachable `return` hands back, written there or through a name each
/// right-hand side of which is one (`const f = …`, the arms of a `cond ? a :
/// b`): a function literal, which reads `f` as [`ValueBody::closure`] binds
/// it; a function of the program, by name (`createImpl` handed back as a
/// value), its own body; or a call of one, what it may return called with
/// [`call_of`]'s arguments, at most [`MAX_CALLS`] calls deep (#383). `None`
/// when a return may be anything else — a parameter, a name a nested body
/// assigns (a call may move it before the return), a call of anything but a
/// program function.
fn returned_closures<'a>(
    program: &Functions<'a>,
    f: &'a crate::ir::FunctionIR,
    call: &ModuleCall,
    depth: usize,
) -> Option<Vec<ValueBody<'a>>> {
    use crate::ir::{Expr, Terminator};
    const MAX_HOPS: usize = 8;
    let cfg = &f.body_cfg;
    let bindings = crate::ir::bindings::local_bindings(cfg);
    let nested = assigned_below(cfg, false);
    // Assigned by `f` or by any body nested in it, the closure's own calls
    // included: what a render reads there is no longer the argument.
    let reassigned = assigned_below(cfg, true);
    let local = |v: &str| bindings.contains_key(v) || f.params.iter().any(|p| p == v);
    // By the site that hands each back: a literal's or a call's id, or past
    // both a function's name.
    let mut found: std::collections::BTreeMap<(u64, &str), Vec<ValueBody<'a>>> =
        std::collections::BTreeMap::new();
    let reachable = cfg.reachable_blocks();
    let mut todo: Vec<(&Expr, usize)> = cfg
        .blocks
        .values()
        .filter(|b| reachable.contains(&b.id))
        .filter_map(|b| match &b.term {
            Terminator::Return(value, _) => Some((value, 0)),
            _ => None,
        })
        .collect();
    let mut seen: HashSet<&str> = HashSet::new();
    while let Some((value, hops)) = todo.pop() {
        match value.peel_ts() {
            Expr::FnLit {
                id,
                params,
                body_cfg,
            } => {
                found.entry((id.0, "")).or_insert_with(|| {
                    vec![ValueBody::closure(f, call, &reassigned, params, body_cfg)]
                });
            }
            Expr::Var(v) if f.params.contains(v) => return None,
            Expr::Var(v) if local(v) => {
                if hops >= MAX_HOPS || nested.contains(v.as_str()) {
                    return None;
                }
                if seen.insert(v.as_str()) {
                    todo.extend(bindings[v.as_str()].iter().map(|e| (*e, hops + 1)));
                }
            }
            Expr::Var(v) => {
                let g = program.named(&f.file, v)?;
                found
                    .entry((u64::MAX, v.as_str()))
                    .or_insert_with(|| vec![ValueBody::function(g)]);
            }
            Expr::Call { id, fn_, args } if depth < MAX_CALLS => {
                let Expr::Var(name) = fn_.peel_ts() else {
                    return None;
                };
                if local(name) {
                    return None;
                }
                let g = program.named(&f.file, name)?;
                if let std::collections::btree_map::Entry::Vacant(slot) = found.entry((id.0, "")) {
                    let inner = call_of(program, f, call, &local, &reassigned, args, g)?;
                    slot.insert(returned_closures(program, g, &inner, depth + 1)?);
                }
            }
            _ => return None,
        }
    }
    let candidates: Vec<ValueBody<'a>> = found.into_values().flatten().collect();
    (!candidates.is_empty()).then_some(candidates)
}

/// The body that runs one of `candidates`, its prelude first, with what a
/// call of each name the preludes bind is. One candidate is that body.
/// Several are a branch on ⊤ to each, its sides
/// [`RunChoice`](crate::ir::cfg::EdgeKind::RunChoice) edges — the factory
/// returned one of them at module time, and every render of the run calls
/// that one (#344) — the body's parameters positional, each candidate
/// binding its own from them. A name a candidate binds that another one binds
/// or reads too is renamed in it, as a splice renames a callee's (`n#c1`):
/// closures capture variables, and the body's exit, where the sides meet,
/// would hand one closure's effects the other's `n`. Each side's allocation
/// and call sites are past the sides before it, as each copy a splice makes
/// is: one closure may be two candidates (`d ? g(1) : g(2)`, #383).
#[allow(clippy::type_complexity)]
fn choice_of(
    candidates: Vec<ValueBody>,
) -> Option<(
    Vec<Var>,
    crate::ir::cfg::CFG,
    HashMap<Var, Option<hook_extractor::ResolvedHookCall>>,
)> {
    use crate::ir::{
        Expr, Stmt,
        cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
        expr::SummaryValue,
        free_vars::compute_free_vars,
    };
    let with_first = |body: &mut CFG, mut first: Vec<Stmt>| -> Option<()> {
        let entry = body.blocks.get_mut(&body.entry)?;
        first.append(&mut entry.stmts);
        entry.stmts = first;
        Some(())
    };
    if let [_] = candidates.as_slice() {
        let c = candidates.into_iter().next()?;
        let mut body = c.body;
        with_first(&mut body, c.prelude)?;
        return Some((c.params, body, c.bound));
    }
    let arity = candidates.iter().map(|c| c.params.len()).max().unwrap_or(0);
    let params: Vec<Var> = (0..arity).map(|i| format!("__closure_arg{i}")).collect();
    let mut merged = CFG {
        entry: 0,
        blocks: std::collections::BTreeMap::new(),
        edges: Vec::new(),
    };
    let binds: Vec<HashSet<Var>> = candidates
        .iter()
        .map(|c| {
            let mut own = crate::ir::bound_vars(&c.body, &c.params);
            own.extend(c.prelude.iter().filter_map(|s| match s {
                Stmt::Let { var, .. } => Some(var.clone()),
                _ => None,
            }));
            own
        })
        .collect();
    let reads: Vec<HashSet<Var>> = candidates
        .iter()
        .map(|c| compute_free_vars(&c.body))
        .collect();
    let renames: Vec<HashMap<Var, Var>> = binds
        .iter()
        .enumerate()
        .map(|(i, own)| {
            own.iter()
                .filter(|v| {
                    (0..binds.len())
                        .any(|j| j != i && (binds[j].contains(*v) || reads[j].contains(*v)))
                })
                .map(|v| {
                    (
                        v.clone(),
                        format!("{v}{}c{i}", crate::ir::splice::SPLICE_MARK),
                    )
                })
                .collect()
        })
        .collect();
    let renamed = |rename: &HashMap<Var, Var>, v: Var| rename.get(&v).cloned().unwrap_or(v);
    let mut bound = HashMap::new();
    let mut arms = Vec::new();
    for (c, rename) in candidates.into_iter().zip(&renames) {
        let next = merged.blocks.keys().next_back().map_or(0, |b| b + 1);
        let sites = crate::ir::Offsets {
            labels: 0,
            ids: crate::ir::alloc_id_span(&merged),
            calls: crate::ir::call_id_span(&merged),
        };
        let body = crate::ir::rename_vars_cfg(c.body, rename);
        let mut body = crate::ir::remap_cfg(body, sites).shifted(next);
        let mut lets: Vec<Stmt> = c
            .prelude
            .into_iter()
            .map(|s| match s {
                Stmt::Let { var, rhs, span } => Stmt::Let {
                    var: renamed(rename, var),
                    rhs,
                    span,
                },
                s => s,
            })
            .collect();
        lets.extend(c.params.iter().zip(&params).map(|(p, arg)| Stmt::Let {
            var: renamed(rename, p.clone()),
            rhs: Expr::Var(arg.clone()),
            span: None,
        }));
        bound.extend(c.bound.into_iter().map(|(v, h)| (renamed(rename, v), h)));
        with_first(&mut body, lets)?;
        arms.push(body.entry);
        merged.blocks.extend(body.blocks);
        merged.edges.extend(body.edges);
    }
    // Choice `j` takes closure `j` or the next choice; the last one takes
    // the last two closures.
    let first = merged.blocks.keys().next_back().map_or(0, |b| b + 1);
    let choices = arms.len() - 1;
    for (j, &arm) in arms[..choices].iter().enumerate() {
        let id = first + j;
        let rest = if j + 1 == choices {
            arms[j + 1]
        } else {
            id + 1
        };
        merged.blocks.insert(
            id,
            BasicBlock {
                id,
                stmts: Vec::new(),
                term: Terminator::Branch {
                    cond: Expr::SummaryVal(SummaryValue::Top),
                    then_: arm,
                    else_: rest,
                    span: None,
                    chain_link: false,
                },
            },
        );
        for to in [arm, rest] {
            merged.edges.push(Edge {
                from: id,
                to,
                kind: EdgeKind::RunChoice,
            });
        }
    }
    merged.entry = first;
    debug_assert!(merged.validate().is_ok(), "{:?}", merged.validate());
    Some((params, merged, bound))
}

/// The hook names `program` defines: its `use*` functions, and the
/// [`HookAlias`](hook_detector::HookAlias)es whose every arm is one of React's
/// effect hooks, which the IR lowers alike, returned with their first arm.
/// Any other alias is not a definition the analysis reads: a call through it
/// stays a hook it does not see. A hook bound to the value of a call of one of
/// the file's functions, or of a function an import resolves to a file of the
/// program, is the file's own too (#342, #344), whether or not that value is a
/// closure the analysis reads.
fn local_hooks(
    program: &Program,
    origins: &HashMap<String, import_resolution::HookOrigin>,
    react_ns: &HashSet<String>,
    file: &Path,
    resolver: &dyn ImportResolver,
) -> (HashSet<String>, Vec<(hook_detector::HookAlias, String)>) {
    let mut names: HashSet<String> = detect_custom_hooks(program)
        .iter()
        .map(|c| c.name.clone())
        .chain(
            hook_values(program, file, resolver)
                .into_iter()
                .map(|(v, _)| v.name),
        )
        .collect();
    let imports = hook_extractor::ImportCtx {
        origins,
        react_ns,
        local_hooks: &names,
        current_file: Some(file),
        bound: hook_extractor::ImportCtx::unbound(),
    };
    let aliases: Vec<(hook_detector::HookAlias, String)> =
        hook_detector::detect_hook_aliases(program)
            .into_iter()
            .filter_map(|alias| {
                let hooks: Option<Vec<String>> =
                    alias.arms.iter().map(|a| imports.react_hook(a)).collect();
                let hooks = hooks?;
                let first = hooks.first()?.clone();
                hooks
                    .iter()
                    .all(|h| hook_extractor::is_effect_hook(h))
                    .then_some((alias, first))
            })
            .collect();
    names.extend(aliases.iter().map(|(a, _)| a.name.clone()));
    (names, aliases)
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
    let (local_hooks, _) = local_hooks(program, &origins, &react_ns, file, resolver);
    let imports = hook_extractor::ImportCtx {
        origins: &origins,
        react_ns: &react_ns,
        local_hooks: &local_hooks,
        current_file: Some(file),
        bound: hook_extractor::ImportCtx::unbound(),
    };
    let forward_ref_targets =
        react_wrappers::ReactWrappers::of(program).forward_ref_targets(program);
    let components = detect_components(program)
        .into_iter()
        .map(|candidate| {
            let ref_passing = match candidate.wrapped {
                Some(react_wrappers::Wrapper::ForwardRef) => RefPassing::Argument,
                _ if forward_ref_targets.contains(candidate.name.as_str()) => RefPassing::Either,
                _ => RefPassing::Prop,
            };
            let (param_names, mut render_cfg) = candidate.build_render_cfg(&ctx);
            let (mut hooks, hook_provenance, mut next_label) =
                extract_hooks(&mut render_cfg, &imports, &ctx.ids);
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
                ref_passing,
                dom_props,
                render_cfg,
                hooks,
                hook_provenance,
                module_consts: module_consts.clone(),
            }
        })
        .collect();
    ctx.name_temps(source, files);
    components
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

    fn lowered_hooks(src: &str) -> Vec<HookIR> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
        lower_custom_hooks(
            &ret.program,
            src,
            Path::new("/t.tsx"),
            &mut FileTable::default(),
        )
    }

    fn entry_lets(h: &HookIR) -> Vec<(String, crate::ir::Expr)> {
        h.body_cfg.blocks[&h.body_cfg.entry]
            .stmts
            .iter()
            .filter_map(|s| match s {
                crate::ir::Stmt::Let { var, rhs, .. } => Some((var.clone(), rhs.clone())),
                _ => None,
            })
            .collect()
    }

    /// #342: the closure the factory returns is the hook's body, its hooks
    /// extracted; the factory's parameter holds the module call's argument
    /// and a local of the factory the closure reads is ⊤.
    #[test]
    fn a_hook_value_is_the_closure_its_factory_returns() {
        let hooks = lowered_hooks(
            "import { useState, useEffect } from \"react\";\n\
             function make(step) {\n  let calls = 0;\n  return (k) => {\n\
             const [n, setN] = useState(0);\n    useEffect(() => { calls++; setN(n + step + k); }, [n]);\n\
             return n;\n  };\n}\nconst useX = make(2);",
        );
        let h = hooks
            .iter()
            .find(|h| h.name == "useX")
            .expect("useX lowered");
        assert_eq!(h.params, vec!["k".to_string()]);
        assert!(matches!(h.hooks[0], crate::ir::HookEntry::State { .. }));
        assert!(matches!(h.hooks[1], crate::ir::HookEntry::Effect { .. }));
        let lets = entry_lets(h);
        assert!(
            lets.iter()
                .any(|(v, e)| v == "step" && matches!(e, crate::ir::Expr::Lit(Prim::Int(2))))
        );
        assert!(lets.iter().any(|(v, e)| v == "calls"
            && matches!(
                e,
                crate::ir::Expr::SummaryVal(crate::ir::expr::SummaryValue::Top)
            )));
    }

    /// A call of the factory's parameter in the closure is a call of the hook
    /// the module call hands it, here a hook of the file.
    #[test]
    fn a_wrapper_s_parameter_call_is_the_hook_its_argument_names() {
        let hooks = lowered_hooks(
            "function useY() { return 1; }\n\
             function wrap(hook) { return (...args) => hook(...args); }\n\
             const useX = wrap(useY);",
        );
        let h = hooks
            .iter()
            .find(|h| h.name == "useX")
            .expect("useX lowered");
        assert!(
            h.hooks
                .iter()
                .any(|e| matches!(e, crate::ir::HookEntry::Custom { name, .. } if name == "useY")),
            "{:?}",
            h.hooks
        );
    }

    /// #344: a factory returning one of several closures, through `return`s
    /// or a `cond ? a : b`, builds a body branching once per run to each,
    /// its parameters positional and each closure binding its own from them.
    #[test]
    fn a_factory_returning_one_of_several_closures_runs_the_one_chosen() {
        use crate::ir::{Expr, Stmt, cfg::EdgeKind};
        let hooks = lowered_hooks(
            "function two(d) { if (d) return (a) => a; return (b, c) => b + c; }\n\
             function pick(d) { const f = d ? () => 1 : () => 2; return f; }\n\
             const useA = two(true);\nconst useB = pick(false);",
        );
        for name in ["useA", "useB"] {
            let h = hooks
                .iter()
                .find(|h| h.name == name)
                .unwrap_or_else(|| panic!("{name} lowered"));
            let choices = h
                .body_cfg
                .edges
                .iter()
                .filter(|e| e.kind == EdgeKind::RunChoice)
                .count();
            assert_eq!(choices, 2, "{name}: one choice, two sides");
            assert!(h.body_cfg.validate().is_ok());
        }
        let h = hooks.iter().find(|h| h.name == "useA").unwrap();
        assert_eq!(h.params, ["__closure_arg0", "__closure_arg1"]);
        let binds = |var: &str, arg: &str| {
            h.body_cfg.blocks.values().any(|b| {
                b.stmts.iter().any(|s| {
                    matches!(s, Stmt::Let { var: v, rhs: Expr::Var(a), .. } if v == var && a == arg)
                })
            })
        };
        assert!(binds("a", "__closure_arg0"));
        assert!(binds("b", "__closure_arg0"));
        assert!(binds("c", "__closure_arg1"));
    }

    /// #344: a name two closures bind is two names, one per closure.
    #[test]
    fn the_closures_of_one_choice_keep_their_names_apart() {
        let hooks = lowered_hooks(
            "function two(d) { if (d) return (k) => { const n = k; return n; };\n\
             return (k) => { const n = 2; const m = n; return m; }; }\n\
             const useA = two(true);",
        );
        let h = hooks.iter().find(|h| h.name == "useA").expect("useA");
        let bound = crate::ir::bound_vars(&h.body_cfg, &h.params);
        for name in ["k#c0", "k#c1", "n#c0", "n#c1", "m"] {
            assert!(bound.contains(name), "{name}: {bound:?}");
        }
        assert!(!bound.contains("n") && !bound.contains("k"), "{bound:?}");
    }

    /// #383: a factory handing back another program call's value is what
    /// that call returns, the callee's parameters bound from the factory's
    /// (its argument as the module call wrote it), through a chain of calls;
    /// one handing back a program function by name is that function.
    #[test]
    fn a_factory_returning_a_call_or_a_function_runs_what_it_hands_back() {
        use crate::ir::{Expr, cfg::EdgeKind};
        let hooks = lowered_hooks(
            "function impl(step) { return () => step; }\n\
             function mid(s) { return impl(s); }\n\
             function create(s) { return mid(s); }\n\
             function counter(k) { return k; }\n\
             function pick() { return counter; }\n\
             const useA = create(2);\nconst useB = pick();",
        );
        let a = hooks.iter().find(|h| h.name == "useA").expect("useA");
        assert!(
            entry_lets(a)
                .iter()
                .any(|(v, e)| v == "step" && matches!(e, Expr::Lit(Prim::Int(2)))),
            "{:?}",
            entry_lets(a)
        );
        let b = hooks.iter().find(|h| h.name == "useB").expect("useB");
        assert_eq!(b.params, ["k"]);
        assert!(
            !b.body_cfg
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::RunChoice)
        );
    }

    /// #383: one closure two calls hand back is two candidates, each its own
    /// argument and its own allocation sites.
    #[test]
    fn one_closure_two_calls_return_is_two_candidates() {
        use crate::ir::{Expr, Stmt};
        let hooks = lowered_hooks(
            "function g(s) { return () => ({ s }); }\n\
             function two(d) { return d ? g(1) : g(2); }\n\
             const useA = two(true);",
        );
        let h = hooks.iter().find(|h| h.name == "useA").expect("useA");
        let lets: Vec<(String, Expr)> = h
            .body_cfg
            .blocks
            .values()
            .flat_map(|b| &b.stmts)
            .filter_map(|s| match s {
                Stmt::Let { var, rhs, .. } => Some((var.clone(), rhs.clone())),
                _ => None,
            })
            .collect();
        for (var, n) in [("s#c0", 1), ("s#c1", 2)] {
            assert!(
                lets.iter()
                    .any(|(v, e)| v == var && matches!(e, Expr::Lit(Prim::Int(k)) if *k == n)),
                "{var}: {lets:?}"
            );
        }
        let mut sites = Vec::new();
        h.body_cfg.for_each_expr(&mut |e| {
            if let Expr::ObjectLit { id, .. } = e {
                sites.push(*id);
            }
        });
        sites.sort();
        sites.dedup();
        assert_eq!(sites.len(), 2, "{sites:?}");
    }

    /// #383: a returned call of a parameter, of a package's function, of a
    /// function calling itself (past the bound), or a returned parameter a
    /// statement may move: no body, the hook stays opaque.
    #[test]
    fn a_returned_call_the_analysis_does_not_follow_builds_no_hook() {
        let hooks = lowered_hooks(
            "import { ext } from \"pkg\";\n\
             function param(b) { return b(); }\n\
             function pkg(s) { return ext(s); }\n\
             function again(n) { return again(n); }\n\
             function moved(h) { if (h) h = () => 0; return h; }\n\
             const useA = param(() => () => 0);\nconst useB = pkg(1);\n\
             const useC = again(1);\nconst useD = moved(1);",
        );
        assert!(
            hooks.is_empty(),
            "{:?}",
            hooks.iter().map(|h| &h.name).collect::<Vec<_>>()
        );
    }

    /// A factory that returns its parameter, a closure or a value, a name
    /// a nested body reassigns, or one imported from a file the resolver does
    /// not find: no body, the hook stays opaque.
    #[test]
    fn a_factory_with_no_returned_closure_builds_no_hook() {
        let hooks = lowered_hooks(
            "function same(h) { return h; }\n\
             function some(d) { if (d) return () => 0; return null; }\n\
             function moved() { let f = () => 0; const g = () => { f = null; }; g(); return f; }\n\
             import { make } from \"./m\";\n\
             const useA = some(true);\nconst useB = same(useA);\nconst useC = make();\n\
             const useD = moved();",
        );
        assert!(
            hooks.is_empty(),
            "{:?}",
            hooks.iter().map(|h| &h.name).collect::<Vec<_>>()
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
