//! Collect the per-file [`ModuleFacts`] — directive prologue and resolved
//! import edges (ADR-026 §1).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

use crate::ir::ModuleFacts;
use crate::ir::module::{AccessorNames, ReExports, ValueRef};
use crate::resolver::ImportResolver;

/// Read `file`'s directive prologue and its resolved module edges.
///
/// Edges are **value** imports only: `import type { T } from "./t"` is erased
/// before the code ever runs, so it carries no runtime environment — counting
/// it would drag server-only modules into the client closure through a
/// types-only reference.
pub fn collect_module_facts(
    program: &Program,
    file: &Path,
    resolver: &dyn ImportResolver,
) -> ModuleFacts {
    let directives = program
        .directives
        .iter()
        .map(|d| d.expression.value.to_string())
        .collect();

    let mut imports = Vec::new();
    let mut compiled_entries = Vec::new();
    let mut push = |specifier: &str| -> Option<PathBuf> {
        let resolved = resolver.resolve(file, specifier)?;
        if !imports.contains(&resolved) {
            imports.push(resolved.clone());
        }
        if resolver.is_compiled_entry(file, specifier) && !compiled_entries.contains(&resolved) {
            compiled_entries.push(resolved.clone());
        }
        Some(resolved)
    };
    // `import { a } from "./x"; export { a }` re-exports `./x`'s `a`.
    let bindings = super::build_resolved_imports(program, file, resolver);
    let mut re_exports = ReExports::default();

    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) if decl.import_kind == ImportOrExportKind::Value => {
                push(decl.source.value.as_str());
            }
            // `export { X } from "./x"` / `export * from "./x"`: a barrel
            // re-export pulls the module in exactly like an import, so it
            // carries a directive boundary the same way.
            Statement::ExportNamedDeclaration(decl)
                if decl.export_kind == ImportOrExportKind::Value =>
            {
                let from = decl.source.as_ref().map(|s| push(s.value.as_str()));
                for spec in &decl.specifiers {
                    if spec.export_kind != ImportOrExportKind::Value {
                        continue;
                    }
                    let exported = spec.exported.name().to_string();
                    let local = spec.local.name().to_string();
                    let origin = match &from {
                        Some(Some(file)) => Some((file.clone(), local)),
                        Some(None) => None,
                        None => bindings
                            .get(&local)
                            .map(|r| (r.file.clone(), r.imported.clone())),
                    };
                    if let Some((file, imported)) = origin {
                        let imported = if imported == "default" {
                            exported.clone()
                        } else {
                            imported
                        };
                        re_exports.named.push((exported, file, imported));
                    }
                }
            }
            Statement::ExportAllDeclaration(decl)
                if decl.export_kind == ImportOrExportKind::Value =>
            {
                let from = push(decl.source.value.as_str());
                // `export * as ns from` exports one namespace object, not names.
                if let Some(from) = from
                    && decl.exported.is_none()
                {
                    re_exports.star.push(from);
                }
            }
            _ => {}
        }
    }
    if !re_exports.star.is_empty() {
        re_exports.declared = super::top_level_binding_names(program)
            .into_iter()
            .collect();
    }

    let mut imported: Vec<(String, PathBuf, String)> = bindings
        .into_iter()
        .map(|(local, origin)| (local, origin.file, origin.imported))
        .collect();
    imported.sort();
    ModuleFacts {
        directives,
        imports,
        compiled_entries,
        re_exports,
        value_refs: value_refs(program, file, resolver),
        accessors: accessor_names(program),
        bindings: super::top_level_binding_names(program),
        imported,
    }
}

/// The member names `program` defines an accessor of, anywhere in it (#334):
/// a `get` or `set` of an object literal or a class, static or not; a
/// descriptor `Object.defineProperty`, `Reflect.defineProperty`,
/// `Object.defineProperties`, `Object.create` or `__defineGetter__` /
/// `__defineSetter__` may install one with; a `Proxy` whose handler may trap
/// `get` or `set`, which runs at a read of any name. A descriptor is an
/// accessor unless it is a literal with no `get` nor `set` member. A key no
/// constant spells may be any name. `accessor x` is left out: its getter and
/// setter only read and write the field's own storage, a data member's
/// behaviour.
///
/// Apart (#336): what may land on an object that exists before it — a
/// descriptor of every one of those calls but `Object.create`, whose
/// descriptors land on the object it creates — and a prototype the program
/// sets (`Object.setPrototypeOf`, `Reflect.setPrototypeOf`, a write of
/// `__proto__`): a literal brings its own accessors, `null` none, anything
/// else any object of the program.
///
/// Apart again (#340): the data members the program may give an object that
/// exists, `Object.prototype` included — a descriptor of any kind, a member of
/// a literal it makes a prototype, a write of a member of a value that may be
/// `Object.prototype` ([`may_be_object_prototype`]), and any member once such
/// a value goes anywhere but into a member access, a comparison or
/// `Object.create`'s prototype.
pub(crate) fn accessor_names(program: &Program) -> AccessorNames {
    let mut scan = AccessorScan {
        found: AccessorNames::scanned(),
        held: HashSet::new(),
    };
    scan.visit_program(program);
    scan.found
}

struct AccessorScan {
    found: AccessorNames,
    /// The spans of the values that may be `Object.prototype` read where
    /// they do not escape: a member access's object, a comparison's operand,
    /// `Object.create`'s prototype.
    held: HashSet<oxc_span::Span>,
}

impl AccessorScan {
    /// `proto` (`None`: not written) is made an existing object's
    /// prototype. A literal puts its own members on that object's chain,
    /// and its own prototype's when it names one; `null` puts none; any other
    /// value may be any object of the program.
    fn prototype(&mut self, proto: Option<&Expression>) {
        match proto.map(Expression::get_inner_expression) {
            Some(Expression::NullLiteral(_)) => {}
            Some(Expression::ObjectExpression(obj)) => {
                for p in &obj.properties {
                    let p = match p {
                        ObjectPropertyKind::ObjectProperty(p) => p,
                        ObjectPropertyKind::SpreadProperty(_) => {
                            self.found.supply(None);
                            continue;
                        }
                    };
                    if p.kind != PropertyKind::Init {
                        self.found.install(key_name(&p.key).as_deref());
                    } else if !p.computed
                        && !p.shorthand
                        && !p.method
                        && key_name(&p.key).is_some_and(|k| k == "__proto__")
                    {
                        self.prototype(Some(&p.value));
                    } else {
                        self.found.supply(key_name(&p.key).as_deref());
                    }
                }
            }
            _ => self.found.reprototype(),
        }
    }

    /// `e` is read where it does not escape.
    fn hold(&mut self, e: &Expression) {
        self.held.insert(e.get_inner_expression().span());
    }
}

/// May `e` be `Object.prototype`? A read of `__proto__` off anything, a
/// call of a `getPrototypeOf`, and `prototype` off `Object`, off a
/// `constructor` read, or off a value no name spells. `Foo.prototype` is not:
/// a name other than `Object` is read as not being it, as an
/// `Object.defineProperty` is recognised by its name (#336).
fn may_be_object_prototype(e: &Expression) -> bool {
    fn named(object: &Expression, key: &str) -> bool {
        match key {
            "__proto__" => true,
            "prototype" => match object.get_inner_expression() {
                Expression::Identifier(id) => id.name == "Object",
                Expression::StaticMemberExpression(m) => {
                    matches!(m.property.name.as_str(), "Object" | "constructor")
                }
                Expression::ComputedMemberExpression(m) => const_name(&m.expression)
                    .is_none_or(|k| matches!(k.as_str(), "Object" | "constructor")),
                _ => true,
            },
            _ => false,
        }
    }
    fn member(m: &MemberExpression) -> bool {
        match m {
            MemberExpression::StaticMemberExpression(s) => named(&s.object, &s.property.name),
            MemberExpression::ComputedMemberExpression(c) => {
                const_name(&c.expression).is_some_and(|k| named(&c.object, &k))
            }
            MemberExpression::PrivateFieldExpression(_) => false,
        }
    }
    fn call(c: &CallExpression) -> bool {
        matches!(
            c.callee.get_inner_expression(),
            Expression::StaticMemberExpression(m) if m.property.name == "getPrototypeOf"
        )
    }
    match e {
        Expression::CallExpression(c) => call(c),
        Expression::ChainExpression(c) => match &c.expression {
            ChainElement::CallExpression(c) => call(c),
            other => other.as_member_expression().is_some_and(member),
        },
        other => other.as_member_expression().is_some_and(member),
    }
}

/// The member name `key` spells, the way lowering spells a read of it.
fn key_name(key: &PropertyKey) -> Option<String> {
    match key {
        PropertyKey::PrivateIdentifier(p) => Some(format!("#{}", p.name)),
        other => super::expr_lower::property_name(other),
    }
}

/// A constant string `e` is, as a member name.
fn const_name(e: &Expression) -> Option<String> {
    match e.get_inner_expression() {
        Expression::StringLiteral(s) => Some(s.value.to_string()),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => t
            .quasis
            .first()
            .and_then(|q| q.value.cooked.as_ref())
            .map(|c| c.to_string()),
        Expression::NumericLiteral(n) if n.value >= 0.0 && n.value.fract() == 0.0 => {
            Some(format!("{}", n.value as u64))
        }
        _ => None,
    }
}

/// May the descriptor (or the handler) `e` hold a `get` or a `set`? Only a
/// literal whose members are all named, none of them those two, holds none.
fn may_hold_accessor(e: Option<&Expression>) -> bool {
    let Some(Expression::ObjectExpression(obj)) = e.map(Expression::get_inner_expression) else {
        return true;
    };
    obj.properties.iter().any(|p| match p {
        ObjectPropertyKind::SpreadProperty(_) => true,
        ObjectPropertyKind::ObjectProperty(p) => {
            key_name(&p.key).is_none_or(|k| k == "get" || k == "set")
        }
    })
}

impl<'a> Visit<'a> for AccessorScan {
    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        if it.kind != PropertyKind::Init {
            self.found.define(key_name(&it.key).as_deref());
        }
        walk::walk_object_property(self, it);
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'a>) {
        if matches!(
            it.kind,
            MethodDefinitionKind::Get | MethodDefinitionKind::Set
        ) {
            self.found.define(key_name(&it.key).as_deref());
        }
        walk::walk_method_definition(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let arg = |i: usize| it.arguments.get(i).and_then(Argument::as_expression);
        if let Expression::StaticMemberExpression(m) = it.callee.get_inner_expression() {
            let on = match m.object.get_inner_expression() {
                Expression::Identifier(id) => Some(id.name.as_str()),
                _ => None,
            };
            // A descriptor call's target gets the members its descriptors
            // name, which the arms below count, and no other.
            if let (Some("Object" | "Reflect"), "defineProperty" | "defineProperties") =
                (on, m.property.name.as_str())
                && let Some(target) = arg(0)
            {
                self.hold(target);
            }
            match (on, m.property.name.as_str()) {
                (Some("Object" | "Reflect"), "defineProperty") => {
                    let member = arg(1).and_then(const_name);
                    match may_hold_accessor(arg(2)) {
                        true => self.found.install(member.as_deref()),
                        false => self.found.supply(member.as_deref()),
                    }
                }
                // The descriptors are the second argument of both;
                // `Object.create`'s land on the object it creates only,
                // whose prototype it only reads.
                (Some("Object"), method @ ("defineProperties" | "create")) => {
                    if method == "create"
                        && let Some(proto) = arg(0)
                    {
                        self.hold(proto);
                    }
                    let mut add = |member: Option<&str>, accessor: bool| match (method, accessor) {
                        ("create", true) => self.found.define(member),
                        ("create", false) => {}
                        (_, true) => self.found.install(member),
                        (_, false) => self.found.supply(member),
                    };
                    match arg(1).map(Expression::get_inner_expression) {
                        Some(Expression::ObjectExpression(obj)) => {
                            for p in &obj.properties {
                                match p {
                                    ObjectPropertyKind::SpreadProperty(_) => add(None, true),
                                    ObjectPropertyKind::ObjectProperty(p) => add(
                                        key_name(&p.key).as_deref(),
                                        may_hold_accessor(Some(&p.value)),
                                    ),
                                }
                            }
                        }
                        // `Object.create(proto)` installs no member.
                        None => {}
                        Some(_) => add(None, true),
                    }
                }
                (Some("Object" | "Reflect"), "setPrototypeOf") => self.prototype(arg(1)),
                (_, "__defineGetter__" | "__defineSetter__") => {
                    self.found.install(arg(0).and_then(const_name).as_deref());
                }
                _ => {}
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        let proto = match &it.left {
            AssignmentTarget::StaticMemberExpression(m) => m.property.name == "__proto__",
            AssignmentTarget::ComputedMemberExpression(m) => {
                const_name(&m.expression).is_some_and(|k| k == "__proto__")
            }
            _ => false,
        };
        if proto {
            self.prototype(Some(&it.right));
        }
        walk::walk_assignment_expression(self, it);
    }

    // A value that may be `Object.prototype` escapes wherever nothing below
    // holds it: bound, passed, stored, returned — any member may then be
    // written on it.
    fn visit_expression(&mut self, it: &Expression<'a>) {
        if may_be_object_prototype(it) && !self.held.contains(&it.span()) {
            self.found.supply(None);
        }
        walk::walk_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.hold(&it.object);
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        self.hold(&it.object);
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_binary_expression(&mut self, it: &BinaryExpression<'a>) {
        if it.operator.is_equality()
            || matches!(it.operator, BinaryOperator::Instanceof | BinaryOperator::In)
        {
            self.hold(&it.left);
            self.hold(&it.right);
        }
        walk::walk_binary_expression(self, it);
    }

    // Every place a member is written: an assignment, an update, a
    // destructuring target, a `for … of` head.
    fn visit_simple_assignment_target(&mut self, it: &SimpleAssignmentTarget<'a>) {
        if let Some(m) = it.as_member_expression()
            && may_be_object_prototype(m.object().get_inner_expression())
        {
            let member = match m {
                MemberExpression::StaticMemberExpression(s) => Some(s.property.name.to_string()),
                MemberExpression::ComputedMemberExpression(c) => const_name(&c.expression),
                MemberExpression::PrivateFieldExpression(p) => Some(format!("#{}", p.field.name)),
            };
            self.found.supply(member.as_deref());
        }
        walk::walk_simple_assignment_target(self, it);
    }

    // `const { prototype: p } = Object` binds what may be `Object.prototype`.
    fn visit_binding_property(&mut self, it: &BindingProperty<'a>) {
        if key_name(&it.key).is_some_and(|k| k == "prototype" || k == "__proto__") {
            self.found.supply(None);
        }
        walk::walk_binding_property(self, it);
    }

    fn visit_assignment_target_property_property(
        &mut self,
        it: &AssignmentTargetPropertyProperty<'a>,
    ) {
        if key_name(&it.name).is_some_and(|k| k == "prototype" || k == "__proto__") {
            self.found.supply(None);
        }
        walk::walk_assignment_target_property_property(self, it);
    }

    fn visit_assignment_target_property_identifier(
        &mut self,
        it: &AssignmentTargetPropertyIdentifier<'a>,
    ) {
        if matches!(it.binding.name.as_str(), "prototype" | "__proto__") {
            self.found.supply(None);
        }
        walk::walk_assignment_target_property_identifier(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        if let Expression::Identifier(id) = it.callee.get_inner_expression()
            && id.name == "Proxy"
            && may_hold_accessor(it.arguments.get(1).and_then(Argument::as_expression))
        {
            self.found.define(None);
        }
        walk::walk_new_expression(self, it);
    }
}

/// The definitions `program` reads as a value (#313): every identifier read
/// outside a JSX element's tag, an `export` clause and a type, resolved to
/// this file's own top-level name or through an import; a default or
/// namespace import, and a dynamic `import()` of a file, stand for any
/// definition of that file. A name bound nowhere at the top level and
/// imported from no analyzed file names no definition of the program.
fn value_refs(program: &Program, file: &Path, resolver: &dyn ImportResolver) -> Vec<ValueRef> {
    let mut bindings: HashMap<String, ValueRef> = HashMap::new();
    for name in super::top_level_binding_names(program) {
        bindings.insert(name.clone(), ValueRef::Named(file.to_path_buf(), name));
    }
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let Some(from) = resolver.resolve(file, decl.source.value.as_str()) else {
            continue;
        };
        for spec in decl.specifiers.iter().flatten() {
            let (local, r) = match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => (
                    s.local.name.to_string(),
                    ValueRef::Named(from.clone(), s.imported.name().to_string()),
                ),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (s.local.name.to_string(), ValueRef::File(from.clone()))
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    (s.local.name.to_string(), ValueRef::File(from.clone()))
                }
            };
            bindings.insert(local, r);
        }
    }
    let mut scan = ValueScan {
        bindings: &bindings,
        file,
        resolver,
        found: HashSet::new(),
    };
    scan.visit_program(program);
    let mut found: Vec<ValueRef> = scan.found.into_iter().collect();
    found.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    found
}

struct ValueScan<'r> {
    bindings: &'r HashMap<String, ValueRef>,
    file: &'r Path,
    resolver: &'r dyn ImportResolver,
    found: HashSet<ValueRef>,
}

impl<'a> Visit<'a> for ValueScan<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if let Some(r) = self.bindings.get(it.name.as_str()) {
            self.found.insert(r.clone());
        }
    }

    // `<C/>`: the element tree places it. A member tag (`<parts.C/>`) reads
    // its object.
    fn visit_jsx_element_name(&mut self, it: &JSXElementName<'a>) {
        if !matches!(it, JSXElementName::IdentifierReference(_)) {
            walk::walk_jsx_element_name(self, it);
        }
    }

    // An export hands the name to importers, whose reads are theirs.
    fn visit_export_specifier(&mut self, _it: &ExportSpecifier<'a>) {}

    fn visit_export_default_declaration_kind(&mut self, it: &ExportDefaultDeclarationKind<'a>) {
        if !matches!(it, ExportDefaultDeclarationKind::Identifier(_)) {
            walk::walk_export_default_declaration_kind(self, it);
        }
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        if let Expression::StringLiteral(s) = &it.source
            && let Some(from) = self.resolver.resolve(self.file, s.value.as_str())
        {
            self.found.insert(ValueRef::File(from));
        }
        walk::walk_import_expression(self, it);
    }

    // Types are erased before anything runs.
    fn visit_ts_type(&mut self, _it: &TSType<'a>) {}
    fn visit_ts_type_annotation(&mut self, _it: &TSTypeAnnotation<'a>) {}
    fn visit_ts_type_parameter_instantiation(&mut self, _it: &TSTypeParameterInstantiation<'a>) {}
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::ImportResolver;
    use std::path::PathBuf;

    /// Resolves any specifier starting with `.` to `/root/<name>.tsx`.
    struct FakeResolver;
    impl ImportResolver for FakeResolver {
        fn resolve(&self, _from: &Path, specifier: &str) -> Option<PathBuf> {
            specifier
                .strip_prefix("./")
                .map(|n| PathBuf::from(format!("/root/{n}.tsx")))
        }
    }

    fn facts_of(source: &str) -> ModuleFacts {
        use oxc_allocator::Allocator;
        use oxc_parser::Parser as OxcParser;
        use oxc_span::SourceType;
        let alloc = Allocator::default();
        let ret = OxcParser::new(&alloc, source, SourceType::tsx()).parse();
        assert!(
            ret.diagnostics.is_empty(),
            "parse errors: {:?}",
            ret.diagnostics
        );
        collect_module_facts(&ret.program, Path::new("/root/a.tsx"), &FakeResolver)
    }

    /// The names spelled, and whether any name may be an accessor's.
    fn accessors_of(source: &str) -> (Vec<String>, bool) {
        let f = facts_of(source);
        let (names, any) = f.accessors.names();
        (names.map(str::to_string).collect(), any)
    }

    #[test]
    fn every_accessor_spelling_defines_its_name() {
        let (names, any) = accessors_of(
            r#"
            const o = { get a() { return 1; }, set b(v) {}, c: 1, d() {} };
            class K { get e() { return 1; } static set f(v) {} get #g() { return 1; } h() {} accessor i = 1; }
            Object.defineProperty(o, "j", { get() { return 1; } });
            Object.defineProperty(o, "k", { value: 1, writable: true });
            Reflect.defineProperty(o, "l", descriptor);
            Object.defineProperties(o, { m: { get: () => 1 }, n: { value: 2 } });
            Object.create(null, { p: { set(v) {} } });
            o.__defineGetter__("q", () => 1);
        "#,
        );
        assert_eq!(names, ["#g", "a", "b", "e", "f", "j", "l", "m", "p", "q"]);
        assert!(!any);
    }

    #[test]
    fn an_accessor_no_constant_names_may_be_any() {
        for src in [
            "const o = { get [k]() { return 1; } };",
            "class K { set [k](v) {} }",
            "Object.defineProperty(o, k, { get() { return 1; } });",
            "Object.defineProperties(o, props);",
            "const p = new Proxy(t, { get: () => 1 });",
            "const p = new Proxy(t, handler);",
        ] {
            assert!(accessors_of(src).1, "{src}");
        }
        let (names, any) = accessors_of("const p = new Proxy(t, { apply: () => 1 });");
        assert!(names.is_empty() && !any);
    }

    /// What a read off a plain object React builds may run (#336): only an
    /// accessor a descriptor installs, or the accessors of a prototype the
    /// program sets.
    #[test]
    fn a_plain_object_reads_only_installed_accessors() {
        use crate::ir::module::AccessKey;
        let off_plain = |src: &str, name: &str| {
            facts_of(src)
                .accessors
                .may_run_off_plain(AccessKey::Named(name))
        };
        // A literal's, a class's, a `Proxy`'s and `Object.create`'s land on
        // another object.
        for src in [
            "const o = { get p() { return 1; } };",
            "class K { get p() { return 1; } }",
            "const t = new Proxy({}, { get: () => ({}) });",
            "Object.create(null, { p: { get: () => 1 } });",
            "Object.setPrototypeOf(o, null); o.__proto__ = { q: 1 };",
        ] {
            assert!(!off_plain(src, "p"), "{src}");
        }
        // A descriptor may install on `Object.prototype` or the object.
        for src in [
            "Object.defineProperty(Object.prototype, \"p\", { get: () => ({}) });",
            "Reflect.defineProperty(t, \"p\", d);",
            "Object.defineProperties(t, { p: { get: () => 1 } });",
            "t.__defineGetter__(\"p\", () => 1);",
            "Object.defineProperty(t, k, { get: () => 1 });",
            "Object.setPrototypeOf(o, { get p() { return 1; } });",
            "o.__proto__ = { __proto__: { set p(v) {} } };",
        ] {
            assert!(off_plain(src, "p"), "{src}");
        }
        assert!(!off_plain(
            "Object.defineProperty(t, \"q\", { get: () => 1 });",
            "p"
        ));
        // A prototype that may be any object brings every accessor.
        let proxy = "const t = new Proxy({}, { get: () => ({}) });";
        for set in ["Object.setPrototypeOf(o, t);", "o[\"__proto__\"] = t;"] {
            assert!(off_plain(&format!("{proxy}\n{set}"), "p"), "{set}");
        }
    }

    /// What a literal that does not hold `f` may read there (#340): only a
    /// member `Object.prototype` holds or one the program may supply to an
    /// object that exists, as data or as an accessor.
    #[test]
    fn a_literal_inherits_only_what_the_program_may_supply() {
        use crate::ir::module::AccessKey;
        let inherits = |src: &str, name: &str| {
            facts_of(src)
                .accessors
                .literal_may_inherit(AccessKey::Named(name))
        };
        for src in [
            "const o = { a: 1 }; o.f = 2;",
            "Foo.prototype.f = () => 1;",
            "window.HTMLElement.prototype.f = () => {};",
            "jest.spyOn(HTMLElement.prototype, 'f');",
            "Object.prototype.hasOwnProperty.call(o, 'f');",
            "const c = Object.getPrototypeOf(o) === Object.prototype;",
            "const k = Object.create(Object.getPrototypeOf(o));",
            "Object.defineProperty(t, \"g\", { value: 1 });",
            "Object.defineProperty(Object.prototype, \"g\", { get: () => 1 });",
            "class K { f() {} get g() { return 1; } }",
            "const { [k]: v } = Object;",
        ] {
            assert!(!inherits(src, "f"), "{src}");
        }
        for name in ["toString", "valueOf", "constructor", "__proto__"] {
            assert!(inherits("", name), "{name}");
        }
        for src in [
            "Object.prototype.f = () => 1;",
            "Object.prototype[\"f\"] = 1;",
            "Object.prototype[k] = 1;",
            "o.__proto__.f = 1;",
            "Object.getPrototypeOf(o).f = 1;",
            "o.constructor.prototype.f = 1;",
            "globalThis.Object.prototype.f++;",
            "({ a: Object.prototype.f } = x);",
            "const p = Object.prototype; p.f = 1;",
            "Object.assign(Object.prototype, { f: 1 });",
            "const { prototype } = Object;",
            "Object.defineProperty(t, \"f\", { value: 1 });",
            "Object.defineProperty(t, k, { value: 1 });",
            "Object.defineProperties(t, { f: { value: 1 } });",
            "Object.defineProperty(Object.prototype, \"f\", { get: () => 1 });",
            "Object.setPrototypeOf(o, { f: 1 });",
            "o.__proto__ = { f() {} };",
            "Object.setPrototypeOf(o, p);",
        ] {
            assert!(inherits(src, "f"), "{src}");
        }
        // Names no scan built know nothing of the program.
        assert!(
            crate::ir::module::AccessorNames::default().literal_may_inherit(AccessKey::Named("f"))
        );
    }

    #[test]
    fn reads_the_directive_prologue() {
        let f = facts_of("'use client';\nexport const A = 1;");
        assert_eq!(f.directives, vec!["use client".to_string()]);
        assert!(f.has_directive("use client"));
    }

    #[test]
    fn double_quotes_and_multiple_directives() {
        let f = facts_of("\"use strict\";\n\"use client\";\nexport const A = 1;");
        assert_eq!(f.directives, vec!["use strict", "use client"]);
    }

    #[test]
    fn a_string_after_real_code_is_not_a_directive() {
        // oxc ends the prologue at the first non-string statement; a stray
        // string expression below it must not read as `"use client"`.
        let f = facts_of("export const A = 1;\n'use client';");
        assert!(f.directives.is_empty());
        assert!(!f.has_directive("use client"));
    }

    #[test]
    fn collects_value_imports_and_re_exports() {
        let f = facts_of(
            r#"
            import { a } from "./a";
            import "./side-effect";
            import def from "./def";
            export { z } from "./z";
            export * from "./star";
            import { pkg } from "some-package";
        "#,
        );
        let names: Vec<String> = f
            .imports
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a", "side-effect", "def", "z", "star"]);
    }

    #[test]
    fn type_only_edges_are_not_runtime_edges() {
        let f = facts_of(
            r#"
            import type { T } from "./types";
            export type { U } from "./more-types";
            import { real } from "./real";
        "#,
        );
        let names: Vec<String> = f
            .imports
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["real"]);
    }

    #[test]
    fn collects_the_re_exports_a_barrel_is_made_of() {
        let f = facts_of(
            r#"
            import { useA } from "./a";
            export { useA as useAA };
            export { useB, default as useC } from "./b";
            export * from "./star";
            export * as ns from "./ns";
            export { useP } from "some-package";
            export function local() {}
        "#,
        );
        let r = &f.re_exports;
        let at = |n: &str| PathBuf::from(format!("/root/{n}.tsx"));
        assert_eq!(
            r.named,
            vec![
                ("useAA".to_string(), at("a"), "useA".to_string()),
                ("useB".to_string(), at("b"), "useB".to_string()),
                ("useC".to_string(), at("b"), "useC".to_string()),
            ]
        );
        assert_eq!(
            r.star,
            vec![at("star")],
            "a namespace re-export is not a star"
        );
        assert!(r.declared.contains("local"));
    }

    #[test]
    fn edges_are_deduped_in_first_seen_order() {
        let f = facts_of("import { a } from \"./x\";\nimport { b } from \"./x\";");
        assert_eq!(f.imports.len(), 1);
    }
}
