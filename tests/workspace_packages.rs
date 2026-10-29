//! Monorepo workspace packages resolve to their source (#48).
//!
//! `@dub/ui` declares `dist/` entries that a source checkout does not hold,
//! so every hook imported from it was opaque. The workspace root's member
//! globs now map each package name to its manifest's entry, and a compiled
//! entry to the package's `src/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use reactant::{
    driver::{CheckOptions, ReportFormat, run_check},
    project::scope_by_project,
    resolver::{DefaultImportResolver, FileSystem, ImportResolver, MemFileSystem},
    rules::RuleRegistry,
};

fn mem_fs(files: &[(&str, &str)]) -> Arc<dyn FileSystem> {
    Arc::new(MemFileSystem::from_map(
        files.iter().map(|(p, s)| (PathBuf::from(p), s.to_string())),
    ))
}

/// The resolver a run over `/w` builds, with the project roots `marked`.
fn resolver(fs: &Arc<dyn FileSystem>, marked: &[&str]) -> Box<dyn ImportResolver> {
    let marked: Vec<PathBuf> = marked.iter().map(PathBuf::from).collect();
    scope_by_project(
        Box::new(DefaultImportResolver::new(fs.clone())),
        &[],
        &marked,
        fs.clone(),
    )
}

fn resolve(r: &dyn ImportResolver, specifier: &str) -> Option<PathBuf> {
    r.resolve(Path::new("/w/apps/web/src/App.tsx"), specifier)
}

const APP: (&str, &str) = ("/w/apps/web/src/App.tsx", "export {};");

/// npm `workspaces` array; `main`/`exports` point at an unbuilt `dist/`, so
/// the package's `src/` stands for it — the `@dub/ui` shape.
#[test]
fn a_dist_entry_resolves_to_the_source_beside_it() {
    let fs = mem_fs(&[
        (
            "/w/package.json",
            r#"{ "workspaces": ["packages/*", "apps/*"] }"#,
        ),
        APP,
        (
            "/w/packages/ui/package.json",
            r#"{ "name": "@acme/ui", "main": "./dist/index.js",
                 "exports": {
                   ".": { "types": "./dist/index.d.ts", "import": "./dist/index.mjs" },
                   "./icons": { "import": "./dist/icons/index.mjs" } } }"#,
        ),
        ("/w/packages/ui/src/index.ts", ""),
        ("/w/packages/ui/src/icons/index.tsx", ""),
    ]);
    let r = resolver(&fs, &[]);
    assert_eq!(
        resolve(r.as_ref(), "@acme/ui"),
        Some(PathBuf::from("/w/packages/ui/src/index.ts"))
    );
    assert_eq!(
        resolve(r.as_ref(), "@acme/ui/icons"),
        Some(PathBuf::from("/w/packages/ui/src/icons/index.tsx")),
        "a subpath export maps to `src/<subpath>`"
    );
    assert_eq!(
        resolve(r.as_ref(), "@acme/ui/hidden"),
        None,
        "a subpath the `exports` map does not list is not exported"
    );
}

/// pnpm's yaml, a yarn `workspaces.packages` object, and a TypeScript
/// condition ahead of the compiled one (`@chakra-ui/*`'s `dev`).
#[test]
fn pnpm_yarn_and_source_conditions() {
    let pnpm = mem_fs(&[
        ("/w/pnpm-workspace.yaml", "packages:\n  - \"packages/**\"\n"),
        ("/w/package.json", r#"{ "name": "root" }"#),
        APP,
        (
            "/w/packages/nested/charts/package.json",
            r#"{ "name": "@acme/charts",
                 "exports": { ".": { "dev": "./src/main.ts",
                                     "import": { "types": "./dist/t.d.ts", "default": "./dist/esm/index.js" } } } }"#,
        ),
        ("/w/packages/nested/charts/src/main.ts", ""),
    ]);
    assert_eq!(
        resolve(resolver(&pnpm, &[]).as_ref(), "@acme/charts"),
        Some(PathBuf::from("/w/packages/nested/charts/src/main.ts"))
    );

    let yarn = mem_fs(&[
        (
            "/w/package.json",
            r#"{ "workspaces": { "packages": ["libs/sdk"] } }"#,
        ),
        APP,
        (
            "/w/libs/sdk/package.json",
            r#"{ "name": "sdk", "exports": { "./*": "./lib/*.js" } }"#,
        ),
        ("/w/libs/sdk/lib/front.js", ""),
    ]);
    assert_eq!(
        resolve(resolver(&yarn, &[]).as_ref(), "sdk/front"),
        Some(PathBuf::from("/w/libs/sdk/lib/front.js")),
        "a pattern export, and a JavaScript entry with no `src/` beside it"
    );
}

/// Soundness: what the workspace does not declare stays unresolved — a
/// package outside the member globs, one a negated glob excludes, a name two
/// members share, and any directory with no workspace above it.
#[test]
fn what_the_workspace_does_not_declare_stays_unresolved() {
    let fs = mem_fs(&[
        (
            "/w/package.json",
            r#"{ "workspaces": ["packages/*", "!packages/legacy"] }"#,
        ),
        APP,
        ("/w/other/pkg/package.json", r#"{ "name": "outside" }"#),
        ("/w/other/pkg/src/index.ts", ""),
        ("/w/packages/legacy/package.json", r#"{ "name": "legacy" }"#),
        ("/w/packages/legacy/src/index.ts", ""),
        ("/w/packages/a/package.json", r#"{ "name": "twin" }"#),
        ("/w/packages/a/src/index.ts", ""),
        ("/w/packages/b/package.json", r#"{ "name": "twin" }"#),
        ("/w/packages/b/src/index.ts", ""),
        ("/elsewhere/App.tsx", ""),
    ]);
    let r = resolver(&fs, &[]);
    for specifier in ["outside", "legacy", "twin", "react"] {
        assert_eq!(resolve(r.as_ref(), specifier), None, "{specifier}");
    }
    assert_eq!(r.resolve(Path::new("/elsewhere/App.tsx"), "twin"), None);
}

/// The project's own statement comes first: a tsconfig `paths` entry for the
/// package name wins over the workspace map.
#[test]
fn tsconfig_paths_win_over_the_workspace() {
    let fs = mem_fs(&[
        ("/w/package.json", r#"{ "workspaces": ["packages/*"] }"#),
        (
            "/w/apps/web/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@acme/ui": ["./vendor/ui.ts"] } } }"#,
        ),
        ("/w/apps/web/vendor/ui.ts", ""),
        APP,
        (
            "/w/packages/ui/package.json",
            r#"{ "name": "@acme/ui", "source": "./src/index.ts" }"#,
        ),
        ("/w/packages/ui/src/index.ts", ""),
    ]);
    assert_eq!(
        resolve(resolver(&fs, &["/w/apps/web"]).as_ref(), "@acme/ui"),
        Some(PathBuf::from("/w/apps/web/vendor/ui.ts"))
    );
    assert_eq!(
        resolve(resolver(&fs, &[]).as_ref(), "@acme/ui"),
        Some(PathBuf::from("/w/packages/ui/src/index.ts")),
        "`source` names the entry"
    );
}

// ── End to end ────────────────────────────────────────────────────────────────

const BUGGY: &str = r#"
import { useState } from "react";

export function useThing() {
  const [n, setN] = useState(0);
  setN(n + 1);
  return n;
}
"#;

const PAGE: &str = r#"
import { useThing } from "@acme/ui";

export function Page() {
  const n = useThing();
  return <div>{n}</div>;
}
"#;

/// Every `(rule, component)` a run over `/w` reports.
fn check(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let opts = CheckOptions {
        info: false,
        show_clean: false,
        trace: false,
        verbose: false,
        all_roots: false,
        entry: vec![],
        exclude_dirs: vec![],
        follow_imports: false,
        format: ReportFormat::Json,
        fail_on: reactant::driver::FailOn::Never,
        project: reactant::driver::ProjectOverride::Auto,
        color: false,
    };
    let out = run_check(
        mem_fs(files),
        &["/w".to_string()],
        &RuleRegistry::natives(),
        &opts,
        &|p| p.to_string_lossy().to_string(),
    );
    let doc: serde_json::Value =
        serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", out.stdout));
    doc["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| {
            (
                d["rule"].as_str().unwrap_or_default().to_string(),
                d["component"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn flags_page(manifest: &str) -> bool {
    check(&[
        (
            "/w/pnpm-workspace.yaml",
            "packages:\n  - apps/*\n  - packages/*\n",
        ),
        ("/w/apps/web/src/Page.tsx", PAGE),
        ("/w/packages/ui/package.json", manifest),
        (
            "/w/packages/ui/src/index.ts",
            "export { useThing } from \"./useThing\";\n",
        ),
        ("/w/packages/ui/src/useThing.ts", BUGGY),
    ])
    .iter()
    .any(|(_, component)| component == "Page")
}

/// The hook behind `@acme/ui` is inlined into the app's component, through
/// the package's barrel; named `other`, the same package stays opaque.
#[test]
fn a_workspace_hook_reaches_the_component_that_calls_it() {
    assert!(flags_page(
        r#"{ "name": "@acme/ui", "exports": { ".": { "import": "./dist/index.mjs" } } }"#
    ));
    assert!(!flags_page(
        r#"{ "name": "other", "main": "./dist/index.js" }"#
    ));
}

/// Whether the App Router's server graph takes `@acme/ui`'s `Panel`, a
/// component calling `useState` with no `"use client"` in its source.
fn panel_is_server_rendered(manifest: &str) -> bool {
    check(&[
        (
            "/w/package.json",
            r#"{ "workspaces": ["apps/*", "packages/*"] }"#,
        ),
        ("/w/apps/web/next.config.js", "module.exports = {};"),
        (
            "/w/apps/web/app/page.tsx",
            "import { Panel } from \"@acme/ui\";\n\
             export default function Home() { return <Panel />; }\n",
        ),
        (
            "/w/apps/web/app/client.tsx",
            "\"use client\";\nexport function C() { return null; }\n",
        ),
        ("/w/packages/ui/package.json", manifest),
        (
            "/w/packages/ui/src/index.ts",
            "export { Panel } from \"./panel\";\n",
        ),
        (
            "/w/packages/ui/src/panel.tsx",
            "import { useState } from \"react\";\n\
             export function Panel() { const [n] = useState(0); return <p>{n}</p>; }\n",
        ),
    ])
    .iter()
    .any(|(rule, component)| rule == "server-component-hook" && component == "Panel")
}

/// A compiled entry's directives are the build's, not the source's (dub's
/// bundler prepends `"use client"` to `@dub/ui`), so the server graph does
/// not enter the source standing in for it; a declared TypeScript entry is
/// the module itself, directives included.
#[test]
fn the_server_graph_does_not_enter_a_compiled_entry() {
    assert!(!panel_is_server_rendered(
        r#"{ "name": "@acme/ui", "main": "./dist/index.mjs" }"#
    ));
    assert!(panel_is_server_rendered(
        r#"{ "name": "@acme/ui", "main": "./src/index.ts" }"#
    ));
}
