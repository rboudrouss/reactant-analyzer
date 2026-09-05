//! End-to-end Next.js project support (ADR-026): detection, router-aware
//! discovery, `baseUrl`/`paths` alias resolution, the `"use client"` module
//! graph, and the `server-component-hook` rule.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use reactant::{
    engine::{Config, ProgramAnalysisResult, RootStrategy},
    project::{self, ProjectKind, server_entry_kind, server_modules},
    registry::SummaryRegistry,
    resolver::{
        DefaultFileDiscoverer, FileDiscoverer, MemFileSystem, OsFileSystem, analyze_files,
        analyze_lowered, lower_files_with,
    },
    rules::{Diagnostic, RuleCtx, Severity, all_rules},
};

/// `MemFileSystem` over `(path, content)` string pairs.
fn mem_fs<const N: usize>(items: [(&str, &str); N]) -> MemFileSystem {
    MemFileSystem::from_map(
        items
            .into_iter()
            .map(|(p, s)| (PathBuf::from(p), s.to_string())),
    )
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/next_project")
}

fn analyze_fixture() -> ProgramAnalysisResult {
    let fs = Arc::new(OsFileSystem);
    let ctx = project::build_context(&fixture_root(), None, fs.clone());
    let files = DefaultFileDiscoverer::new(fs).discover(&ctx.discovery_root);
    let config = Config {
        summary_registry: SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    analyze_files(
        &files,
        ctx.resolver.as_ref(),
        RootStrategy::AllComponents,
        config,
    )
    .0
}

fn findings(result: &ProgramAnalysisResult, component: &str, rule: &str) -> Vec<Diagnostic> {
    let ctx = RuleCtx::new(result, result.component_named(component).unwrap());
    all_rules()
        .iter()
        .flat_map(|r| r.check(&ctx))
        .filter(|d| d.rule == rule)
        .collect()
}

// ── Detection and discovery ───────────────────────────────────────────────────

#[test]
fn next_fixture_is_detected() {
    assert_eq!(
        project::detect(&fixture_root(), &OsFileSystem),
        ProjectKind::NextJs
    );
}

#[test]
fn next_beats_vite_when_both_configs_are_present() {
    // A Next app may keep a vite.config for its test runner; the router
    // conventions are the ones that govern the sources.
    let fs = mem_fs([
        ("/app/next.config.ts", "export default {}"),
        ("/app/vite.config.ts", "export default {}"),
    ]);
    assert_eq!(project::detect(Path::new("/app"), &fs), ProjectKind::NextJs);
}

#[test]
fn discovery_narrows_to_src_only_when_the_router_lives_there() {
    let ctx = project::build_context(&fixture_root(), None, Arc::new(OsFileSystem));
    assert_eq!(ctx.kind, ProjectKind::NextJs);
    assert_eq!(ctx.discovery_root, fixture_root().join("src"));
    assert!(ctx.alias_warning.is_none(), "{:?}", ctx.alias_warning);

    // Router at the root: narrowing would hide the whole app.
    let fs = Arc::new(mem_fs([
        ("/r/next.config.js", "module.exports = {}"),
        (
            "/r/app/page.tsx",
            "export default function P() { return <p/>; }",
        ),
        ("/r/src/util.ts", "export const x = 1;"),
    ]));
    let ctx = project::build_context(Path::new("/r"), None, fs);
    assert_eq!(ctx.discovery_root, Path::new("/r"));
}

// ── Alias resolution ──────────────────────────────────────────────────────────

#[test]
fn tsconfig_paths_alias_resolves() {
    let ctx = project::build_context(&fixture_root(), None, Arc::new(OsFileSystem));
    assert_eq!(
        ctx.resolver.resolve(
            &fixture_root().join("src/app/page.tsx"),
            "@/components/counter"
        ),
        Some(fixture_root().join("src/components/counter.tsx")),
    );
}

#[test]
fn bare_base_url_specifiers_resolve_without_paths() {
    // The Next scaffold that addresses its own tree through `baseUrl` alone
    // (`import "lib/api"`) — vercel/commerce's shape.
    let fs = Arc::new(mem_fs([
        ("/r/next.config.ts", "export default {}"),
        (
            "/r/tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": "." } }"#,
        ),
        (
            "/r/app/page.tsx",
            "export default function P() { return <p/>; }",
        ),
        ("/r/lib/api.ts", "export const get = () => 1;"),
    ]));
    let ctx = project::build_context(Path::new("/r"), None, fs);
    assert_eq!(
        ctx.resolver
            .resolve(Path::new("/r/app/page.tsx"), "lib/api"),
        Some(PathBuf::from("/r/lib/api.ts")),
    );
    // An npm package has no file behind it, so the probe declines.
    assert_eq!(
        ctx.resolver.resolve(Path::new("/r/app/page.tsx"), "react"),
        None
    );
    // `baseUrl` without `paths` is still an alias blind spot worth saying.
    assert!(ctx.alias_warning.is_some());
}

// ── Server/client module graph ────────────────────────────────────────────────

#[test]
fn entry_conventions_need_the_app_dir() {
    assert_eq!(
        server_entry_kind(Path::new("/r/app/page.tsx")),
        Some("page")
    );
    assert_eq!(server_entry_kind(Path::new("/r/lib/page.tsx")), None);
    // Next requires error boundaries to be Client Components.
    assert_eq!(server_entry_kind(Path::new("/r/app/error.tsx")), None);
}

#[test]
fn the_module_table_records_directives_and_edges() {
    let result = analyze_fixture();
    let table = &result.module_table;
    assert!(table.any_declares("use client"));

    let counter = fixture_root().join("src/components/counter.tsx");
    assert!(table.facts(&counter).unwrap().has_directive("use client"));

    // Aliased imports become edges: `@/components/sidebar` from the layout.
    let layout = fixture_root().join("src/app/layout.tsx");
    assert!(
        table
            .facts(&layout)
            .unwrap()
            .imports
            .contains(&fixture_root().join("src/components/sidebar.tsx")),
        "an aliased import must produce a module edge",
    );
}

#[test]
fn the_server_graph_stops_at_use_client() {
    let result = analyze_fixture();
    let server = server_modules(&result.module_table);

    for rel in [
        "src/app/page.tsx",
        "src/components/sidebar.tsx",
        "src/lib/stats.ts",
    ] {
        assert!(
            server.contains(&fixture_root().join(rel)),
            "{rel} is server-compiled"
        );
    }
    assert!(
        !server.contains(&fixture_root().join("src/components/counter.tsx")),
        "a \"use client\" module is not in the server graph",
    );
}

// ── The rule ──────────────────────────────────────────────────────────────────

#[test]
fn a_hook_in_an_app_router_page_is_reported() {
    let result = analyze_fixture();
    let diags = findings(&result, "HomePage", "server-component-hook");
    assert_eq!(diags.len(), 1, "one finding per component, not per hook");
    assert_eq!(diags[0].severity(), Severity::Warning);
    assert!(
        diags[0].message.contains("`useState`"),
        "{}",
        diags[0].message
    );
    assert!(
        diags[0].message.contains("App Router `page`"),
        "{}",
        diags[0].message
    );
}

#[test]
fn a_hook_reached_transitively_from_the_router_is_reported() {
    let result = analyze_fixture();
    let diags = findings(&result, "Sidebar", "server-component-hook");
    assert_eq!(diags.len(), 1);
    // `usePathname` directly, and `useState` through the inlined custom hook.
    assert!(
        diags[0].message.contains("`usePathname`"),
        "{}",
        diags[0].message
    );
    assert!(
        diags[0].message.contains("`useState`"),
        "{}",
        diags[0].message
    );
    assert!(
        diags[0].message.contains("server graph"),
        "the transitive wording, not the entry one: {}",
        diags[0].message
    );
}

#[test]
fn a_client_component_is_not_reported() {
    let result = analyze_fixture();
    assert!(findings(&result, "Counter", "server-component-hook").is_empty());
    // …and the rule does not shadow the real bug in it.
    assert_eq!(findings(&result, "Counter", "infinite-loop").len(), 1);
}

#[test]
fn a_project_without_the_directive_is_never_flagged() {
    // The gate: with no `"use client"` anywhere this is not an RSC codebase,
    // and "Server Component" is not a claim the file layout can support.
    let fs = Arc::new(mem_fs([
        ("/r/next.config.ts", "export default {}"),
        (
            "/r/app/page.tsx",
            "import { useState } from 'react';\n\
             export default function P() { const [n] = useState(0); return <p>{n}</p>; }",
        ),
    ]));
    let ctx = project::build_context(Path::new("/r"), None, fs.clone());
    let files = DefaultFileDiscoverer::new(fs.clone()).discover(&ctx.discovery_root);
    let lowered = lower_files_with(fs.as_ref(), &files, ctx.resolver.as_ref());
    let result = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());
    assert!(!result.module_table.any_declares("use client"));
    assert!(findings(&result, "P", "server-component-hook").is_empty());
}

#[test]
fn next_navigation_hooks_are_known_not_unknown() {
    // `usePathname` in a *client* component: a registered summary, so no
    // `analysis-limit/unknown-hook` Info.
    let result = analyze_fixture();
    let ctx = RuleCtx::new(&result, result.component_named("Counter").unwrap());
    assert!(
        !all_rules()
            .iter()
            .flat_map(|r| r.check(&ctx))
            .any(|d| d.message.contains("not found in registry")),
    );
}
