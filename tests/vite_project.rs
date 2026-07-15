//! End-to-end Vite project support (ADR-016): detection, tsconfig `paths`
//! loading through `references`, and alias-resolved cross-file hook inlining.

use std::path::{Path, PathBuf};

use reactant::{
    engine::{Config, RootStrategy},
    project::{self, ProjectKind},
    resolver::{DefaultFileDiscoverer, FileDiscoverer, analyze_files},
    rules::{Severity, all_rules},
};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vite_project")
}

#[test]
fn vite_fixture_is_detected() {
    assert_eq!(project::detect(&fixture_root()), ProjectKind::Vite);
}

#[test]
fn context_narrows_discovery_to_src_and_loads_aliases() {
    let ctx = project::build_context(&fixture_root(), None);
    assert_eq!(ctx.kind, ProjectKind::Vite);
    assert_eq!(ctx.discovery_root, fixture_root().join("src"));
    assert!(
        ctx.alias_warning.is_none(),
        "tsconfig paths must load through the references hop: {:?}",
        ctx.alias_warning
    );
}

#[test]
fn alias_resolves_through_references_chain() {
    let ctx = project::build_context(&fixture_root(), None);
    let from = fixture_root().join("src/App.tsx");
    let resolved = ctx.resolver.resolve(&from, "@/hooks/useData");
    assert_eq!(
        resolved,
        Some(fixture_root().join("src/hooks/useData.ts")),
        "`@/hooks/useData` must resolve via tsconfig.app.json paths"
    );
}

#[test]
fn infinite_loop_surfaces_on_app_through_alias() {
    // The full pipeline: vite context → src/ discovery → alias-resolved
    // lowering → useData inlined into App's fixpoint → bug on the call site.
    let ctx = project::build_context(&fixture_root(), None);
    let files = DefaultFileDiscoverer.discover(&ctx.discovery_root);
    assert_eq!(files.len(), 2, "App.tsx + hooks/useData.ts");

    let (result, file_count) = analyze_files(
        &files,
        ctx.resolver.as_ref(),
        RootStrategy::Heuristic,
        Config::default(),
    );
    assert_eq!(file_count, 2);
    assert!(result.components.contains_key("App"));

    let diags: Vec<_> = all_rules()
        .iter()
        .flat_map(|r| r.check(&result, &"App".to_string()))
        .collect();
    assert!(
        diags
            .iter()
            .any(|d| d.rule == "infinite-loop" && d.severity == Severity::Warning),
        "infinite-loop must surface on App via the @/ alias; got: {:?}",
        diags.iter().map(|d| d.rule).collect::<Vec<_>>()
    );
}
