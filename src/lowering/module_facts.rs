//! Collect the per-file [`ModuleFacts`] — directive prologue and resolved
//! import edges (ADR-026 §1).

use std::path::Path;

use oxc_ast::ast::{ImportOrExportKind, Program, Statement};

use crate::ir::ModuleFacts;
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
    let mut push = |specifier: &str| {
        if let Some(resolved) = resolver.resolve(file, specifier)
            && !imports.contains(&resolved)
        {
            imports.push(resolved);
        }
    };

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
                if let Some(source) = &decl.source {
                    push(source.value.as_str());
                }
            }
            Statement::ExportAllDeclaration(decl)
                if decl.export_kind == ImportOrExportKind::Value =>
            {
                push(decl.source.value.as_str());
            }
            _ => {}
        }
    }

    ModuleFacts {
        directives,
        imports,
    }
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
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        collect_module_facts(&ret.program, Path::new("/root/a.tsx"), &FakeResolver)
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
    fn edges_are_deduped_in_first_seen_order() {
        let f = facts_of("import { a } from \"./x\";\nimport { b } from \"./x\";");
        assert_eq!(f.imports.len(), 1);
    }
}
