use oxc_ast::ast::*;

use super::Candidate;
use super::jsx_detect::body_returns_jsx;

/// A React component function detected in the AST, ready for lowering.
pub type ComponentCandidate<'a> = Candidate<'a>;

/// Detect all React component functions in `program`.
pub fn detect_components<'a>(program: &'a Program<'a>) -> Vec<ComponentCandidate<'a>> {
    let mut out = Vec::new();
    for stmt in &program.body {
        collect_from_stmt(stmt, &mut out);
    }
    out
}

// ── Top-level statement dispatch ──────────────────────────────────────────────

fn collect_from_stmt<'a>(stmt: &'a Statement<'a>, out: &mut Vec<ComponentCandidate<'a>>) {
    match stmt {
        Statement::FunctionDeclaration(func) => try_add_fn(func, None, out),
        Statement::VariableDeclaration(decl) => {
            for vd in &decl.declarations {
                try_add_var_decl(vd, out);
            }
        }
        Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                let name = func
                    .id
                    .as_ref()
                    .map(|id| id.name.as_str())
                    .unwrap_or("DefaultExport");
                try_add_fn(func, Some(name), out);
            }
            ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) => {
                try_add_arrow("DefaultExport", arrow, None, out);
            }
            _ => {}
        },
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(decl) = &exp.declaration {
                collect_from_decl(decl, out);
            }
        }
        _ => {}
    }
}

fn collect_from_decl<'a>(decl: &'a Declaration<'a>, out: &mut Vec<ComponentCandidate<'a>>) {
    match decl {
        Declaration::FunctionDeclaration(func) => try_add_fn(func, None, out),
        Declaration::VariableDeclaration(vd) => {
            for vd in &vd.declarations {
                try_add_var_decl(vd, out);
            }
        }
        _ => {}
    }
}

// ── Candidate construction ─────────────────────────────────────────────────────

fn try_add_fn<'a>(
    func: &'a Function<'a>,
    name_override: Option<&str>,
    out: &mut Vec<ComponentCandidate<'a>>,
) {
    let name =
        name_override.unwrap_or_else(|| func.id.as_ref().map(|id| id.name.as_str()).unwrap_or(""));
    if name.is_empty() {
        return;
    }
    let Some(body) = func.body.as_deref() else {
        return;
    };
    if is_component(name, &body.statements, func.return_type.as_deref()) {
        out.push(Candidate {
            name: name.to_owned(),
            params: &func.params,
            body,
        });
    }
}

fn try_add_var_decl<'a>(vd: &'a VariableDeclarator<'a>, out: &mut Vec<ComponentCandidate<'a>>) {
    let name = match &vd.id {
        BindingPattern::BindingIdentifier(id) => id.name.as_str(),
        _ => return,
    };
    // Type annotation on `const Foo: React.FC = ...`
    let var_type_ann = vd.type_annotation.as_deref();
    let Some(init) = &vd.init else { return };
    match init {
        Expression::ArrowFunctionExpression(arrow) => {
            try_add_arrow(name, arrow, var_type_ann, out);
        }
        Expression::FunctionExpression(func) => {
            let Some(body) = func.body.as_deref() else {
                return;
            };
            let return_type = func.return_type.as_deref().or(var_type_ann);
            if is_component(name, &body.statements, return_type) {
                out.push(Candidate {
                    name: name.to_owned(),
                    params: &func.params,
                    body,
                });
            }
        }
        _ => {}
    }
}

fn try_add_arrow<'a>(
    name: &str,
    arrow: &'a ArrowFunctionExpression<'a>,
    extra_type_ann: Option<&'a TSTypeAnnotation<'a>>,
    out: &mut Vec<ComponentCandidate<'a>>,
) {
    let return_type = arrow.return_type.as_deref().or(extra_type_ann);
    if is_component(name, &arrow.body.statements, return_type) {
        out.push(Candidate {
            name: name.to_owned(),
            params: &arrow.params,
            body: &arrow.body,
        });
    }
}

// ── Detection rules ────────────────────────────────────────────────────────────

fn is_component(name: &str, stmts: &[Statement], return_type: Option<&TSTypeAnnotation>) -> bool {
    // Rule 1: `use` prefix → hook, never a component
    if name.starts_with("use") {
        return false;
    }
    // React convention: component names must start with uppercase
    if !name.chars().next().is_some_and(|c| c.is_uppercase()) {
        return false;
    }
    // Rule 2: any return path yields JSX
    if body_returns_jsx(stmts) {
        return true;
    }
    // Rule 3: TypeScript component type annotation on the return type
    if let Some(ann) = return_type
        && ts_type_is_component(&ann.type_annotation)
    {
        return true;
    }
    false
}

// ── TypeScript annotation check ───────────────────────────────────────────────

fn ts_type_is_component(ty: &TSType) -> bool {
    match ty {
        TSType::TSTypeReference(tr) => ts_type_name_is_component(&tr.type_name),
        _ => false,
    }
}

fn ts_type_name_is_component(name: &TSTypeName) -> bool {
    match name {
        TSTypeName::QualifiedName(qn) => {
            let right = qn.right.name.as_str();
            let left = match &qn.left {
                TSTypeName::IdentifierReference(id) => id.name.as_str(),
                _ => return false,
            };
            matches!(
                (left, right),
                ("React", "FC")
                    | ("React", "FunctionComponent")
                    | ("React", "ReactElement")
                    | ("React", "ReactNode")
                    | ("JSX", "Element")
            )
        }
        TSTypeName::IdentifierReference(id) => {
            matches!(
                id.name.as_str(),
                "ReactElement" | "FC" | "FunctionComponent"
            )
        }
        TSTypeName::ThisExpression(_) => false,
    }
}

// ── DOM-typed props ───────────────────────────────────────────────────────────

/// Names of props whose declared TypeScript type is a DOM interface
/// (`canvas: HTMLCanvasElement`). Looks through the props parameter's
/// annotation: inline type literal, or a same-file `type`/`interface`
/// declaration referenced by name. Cross-file props types are not resolved —
/// missing an exemption only costs a Warning-level advice, never a hidden bug.
pub fn collect_dom_props(
    params: &FormalParameters,
    program: &Program,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Some(first) = params.items.first() else {
        return out;
    };
    let Some(ann) = &first.type_annotation else {
        return out;
    };
    match &ann.type_annotation {
        TSType::TSTypeLiteral(lit) => collect_dom_members(&lit.members, &mut out),
        TSType::TSTypeReference(tr) => {
            if let TSTypeName::IdentifierReference(id) = &tr.type_name {
                resolve_dom_members(program, id.name.as_str(), &mut out);
            }
        }
        _ => {}
    }
    out
}

/// Find a same-file `type X = {…}` / `interface X {…}` and collect its
/// DOM-typed members.
fn resolve_dom_members(program: &Program, name: &str, out: &mut std::collections::HashSet<String>) {
    for stmt in &program.body {
        let decl = match stmt {
            Statement::TSTypeAliasDeclaration(d) => Some(TypeDecl::Alias(d)),
            Statement::TSInterfaceDeclaration(d) => Some(TypeDecl::Interface(d)),
            Statement::ExportNamedDeclaration(exp) => match &exp.declaration {
                Some(Declaration::TSTypeAliasDeclaration(d)) => Some(TypeDecl::Alias(d)),
                Some(Declaration::TSInterfaceDeclaration(d)) => Some(TypeDecl::Interface(d)),
                _ => None,
            },
            _ => None,
        };
        match decl {
            Some(TypeDecl::Alias(d)) if d.id.name == name => {
                if let TSType::TSTypeLiteral(lit) = &d.type_annotation {
                    collect_dom_members(&lit.members, out);
                }
                return;
            }
            Some(TypeDecl::Interface(d)) if d.id.name == name => {
                collect_dom_members(&d.body.body, out);
                return;
            }
            _ => {}
        }
    }
}

enum TypeDecl<'a, 'b> {
    Alias(&'b TSTypeAliasDeclaration<'a>),
    Interface(&'b TSInterfaceDeclaration<'a>),
}

fn collect_dom_members(members: &[TSSignature], out: &mut std::collections::HashSet<String>) {
    for m in members {
        if let TSSignature::TSPropertySignature(p) = m
            && let Some(key) = static_key_name(&p.key)
            && let Some(ann) = &p.type_annotation
            && ts_type_is_dom(&ann.type_annotation)
        {
            out.insert(key);
        }
    }
}

fn static_key_name(key: &PropertyKey) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
        PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
        _ => None,
    }
}

fn ts_type_is_dom(ty: &TSType) -> bool {
    if let TSType::TSTypeReference(tr) = ty
        && let TSTypeName::IdentifierReference(id) = &tr.type_name
    {
        let n = id.name.as_str();
        return ((n.starts_with("HTML") || n.starts_with("SVG")) && n.ends_with("Element"))
            || matches!(
                n,
                "Element" | "HTMLElement" | "SVGElement" | "Node" | "EventTarget" | "Document"
            );
    }
    false
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn names(src: &str) -> Vec<String> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        detect_components(&ret.program)
            .into_iter()
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn fn_declaration_with_jsx() {
        assert_eq!(
            names("function Counter() { return <div/>; }"),
            vec!["Counter"]
        );
    }

    #[test]
    fn hook_excluded() {
        assert!(names("function useCount() { return <div/>; }").is_empty());
    }

    #[test]
    fn lowercase_excluded() {
        assert!(names("function helper() { return <div/>; }").is_empty());
    }

    #[test]
    fn arrow_expression_body() {
        assert_eq!(
            names("const Button = () => <button>ok</button>;"),
            vec!["Button"]
        );
    }

    #[test]
    fn arrow_block_body() {
        assert_eq!(
            names("const Card = () => { return <div/>; };"),
            vec!["Card"]
        );
    }

    #[test]
    fn export_default_fn() {
        assert_eq!(
            names("export default function App() { return <main/>; }"),
            vec!["App"]
        );
    }

    #[test]
    fn export_named_fn() {
        assert_eq!(
            names("export function Header() { return <header/>; }"),
            vec!["Header"]
        );
    }

    #[test]
    fn conditional_jsx_return() {
        let src = "function Comp({ ok }) { if (ok) return <div/>; return null; }";
        assert_eq!(names(src), vec!["Comp"]);
    }

    #[test]
    fn ts_react_fc_annotation() {
        // Rule 3: component type annotation, no JSX in body
        let src = "function Stub(): React.ReactElement { return null as any; }";
        assert_eq!(names(src), vec!["Stub"]);
    }

    #[test]
    fn var_react_fc_annotation() {
        let src = "const Stub: React.FC = () => null as any;";
        assert_eq!(names(src), vec!["Stub"]);
    }
}
