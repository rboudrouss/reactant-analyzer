//! A context declared in one file and provided in another.
//!
//! `collect_module_consts` sees a single file, so `import { Ctx } from "./ctx"`
//! left `Ctx` unproven and `<Ctx.Provider>` invisible to the provider relation.
//! The proof is completed once every file is lowered
//! (`resolver::lower_files_with`), keyed by the name the *origin* exports —
//! not the local one, so an aliased import still resolves.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::{
    driver::{CheckOptions, run_check},
    resolver::OsFileSystem,
    rules::RuleRegistry,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-xfilectx-{}-{}-{}",
            std::process::id(),
            label,
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create tmp dir");
        Tmp(path)
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        fs::write(&path, body).expect("write file");
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run `check --all-roots --format json` over the directory and return the
/// `unstable-context-value` findings' `(file, line)` pairs.
fn provider_findings(tmp: &Tmp) -> Vec<(String, u64)> {
    let opts = CheckOptions {
        info: false,
        show_clean: false,
        trace: false,
        verbose: false,
        all_roots: true,
        entry: vec![],
        exclude_dirs: vec![],
        format: reactant::driver::ReportFormat::Json,
        fail_on: reactant::driver::FailOn::Error,
        project: reactant::driver::ProjectOverride::Auto,
        color: false,
    };
    let paths = vec![tmp.0.to_string_lossy().to_string()];
    let out = run_check(
        std::sync::Arc::new(OsFileSystem),
        &paths,
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
        .filter(|d| d["rule"] == "unstable-context-value")
        .map(|d| {
            (
                d["file"].as_str().unwrap_or_default().to_string(),
                d["line"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

const CTX_FILE: &str = r#"
import { createContext } from "react";
export const SharedContext = createContext(null);
export const Renamed = createContext(null);
"#;

#[test]
fn an_imported_context_is_proven_and_its_fresh_value_reported() {
    let tmp = Tmp::new("plain");
    tmp.write("ctx.ts", CTX_FILE);
    tmp.write(
        "App.tsx",
        r#"
import { useState } from "react";
import { SharedContext } from "./ctx";

export function Consumer({ children }) {
  const [user, setUser] = useState(null);
  return <SharedContext.Provider value={{ user, setUser }}>{children}</SharedContext.Provider>;
}
"#,
    );
    let found = provider_findings(&tmp);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].0.ends_with("App.tsx"), "{found:?}");
}

/// The proof keys on the exported name: a local alias must still resolve, which
/// a local-name-only import map cannot do.
#[test]
fn an_aliased_imported_context_is_proven() {
    let tmp = Tmp::new("alias");
    tmp.write("ctx.ts", CTX_FILE);
    tmp.write(
        "App.tsx",
        r#"
import { useState } from "react";
import { Renamed as Aliased } from "./ctx";

export function Consumer({ children }) {
  const [n, setN] = useState(0);
  return <Aliased.Provider value={{ n, setN }}>{children}</Aliased.Provider>;
}
"#,
    );
    assert_eq!(provider_findings(&tmp).len(), 1);
}

/// A namespace-imported component that merely *has* a `.Provider` is not a
/// React context — Radix, Jotai and friends all ship one. Two of the fourteen
/// cross-file `.Provider` elements on the corpus are this shape, so the
/// two-valued proof is load-bearing, not defensive.
#[test]
fn a_namespace_component_provider_is_not_a_context() {
    let tmp = Tmp::new("namespace");
    tmp.write(
        "tooltip.tsx",
        r#"
import { useState } from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";

export function TooltipProvider({ children }) {
  const [n] = useState(0);
  return <TooltipPrimitive.Provider value={{ n }}>{children}</TooltipPrimitive.Provider>;
}
"#,
    );
    assert_eq!(provider_findings(&tmp).len(), 0);
}

/// The context lives in a file with no component of its own — the common shape
/// (`contexts/ctx.ts` exports only the context), and the reason the per-file
/// scan cannot be derived from the analysed components alone.
#[test]
fn a_context_file_without_components_still_proves() {
    let tmp = Tmp::new("no-component");
    tmp.write(
        "contexts/only-context.ts",
        r#"
import { createContext } from "react";
export const Bare = createContext(null);
"#,
    );
    tmp.write(
        "App.tsx",
        r#"
import { useState } from "react";
import { Bare } from "./contexts/only-context";

export function P({ children }) {
  const [n, setN] = useState(0);
  return <Bare.Provider value={{ n, setN }}>{children}</Bare.Provider>;
}
"#,
    );
    assert_eq!(provider_findings(&tmp).len(), 1);
}

// ── #109: the identity the resolver used to discard ──────────────────────────

/// Two files importing the same cell under different local names must resolve
/// to the SAME [`ContextId`] — that is what a cross-file consumer↔provider
/// pairing keys on, and what the unit variant made impossible.
#[test]
fn the_same_cell_imported_twice_has_one_canonical_identity() {
    use reactant::engine::{Config, RootStrategy};
    use reactant::ir::ModuleConstInit;
    use reactant::resolver::{DefaultImportResolver, analyze_lowered, lower_files};

    let tmp = Tmp::new("identity");
    tmp.write(
        "ctx.ts",
        "import { createContext } from \"react\";\nexport const TabsContext = createContext(null);\n",
    );
    tmp.write(
        "a.tsx",
        "import { TabsContext } from \"./ctx\";\nexport function A() {\n  return <TabsContext.Provider value={{}}><div/></TabsContext.Provider>;\n}\n",
    );
    tmp.write(
        "b.tsx",
        "import { TabsContext as Tabs } from \"./ctx\";\nexport function B() {\n  return <Tabs.Provider value={{}}><div/></Tabs.Provider>;\n}\n",
    );

    let files: Vec<PathBuf> = ["ctx.ts", "a.tsx", "b.tsx"]
        .iter()
        .map(|f| tmp.0.join(f))
        .collect();
    let lowered = lower_files(&files, &DefaultImportResolver::default());
    assert!(
        lowered.parse_errors.is_empty(),
        "{:?}",
        lowered.parse_errors
    );
    let prog = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());

    let id_of = |comp: &str, local: &str| {
        let c = prog
            .components
            .get(comp)
            .unwrap_or_else(|| panic!("no component {comp}"));
        match c.module_consts.get(local) {
            Some(ModuleConstInit::Context(id)) => id.clone(),
            other => panic!("{comp}.{local} is not a proven context: {other:?}"),
        }
    };

    let a = id_of("A", "TabsContext");
    let b = id_of("B", "Tabs");
    assert_eq!(a, b, "the local alias must not be what identifies the cell");
    assert_eq!(
        a.origin_name, "TabsContext",
        "the EXPORTED name, not the alias"
    );
    assert!(
        a.origin_file.ends_with("ctx.ts"),
        "the defining file: {:?}",
        a.origin_file
    );
}

/// A locally-created context is its own origin.
#[test]
fn a_local_context_identifies_itself() {
    use reactant::engine::{Config, RootStrategy};
    use reactant::ir::ModuleConstInit;
    use reactant::resolver::{DefaultImportResolver, analyze_lowered, lower_files};

    let tmp = Tmp::new("local-identity");
    tmp.write(
        "own.tsx",
        "import { createContext } from \"react\";\nconst Own = createContext(null);\nexport function C() {\n  return <Own.Provider value={{}}><div/></Own.Provider>;\n}\n",
    );
    let files = vec![tmp.0.join("own.tsx")];
    let lowered = lower_files(&files, &DefaultImportResolver::default());
    let prog = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());
    let c = &prog.components[&"C".to_string()];
    match c.module_consts.get("Own") {
        Some(ModuleConstInit::Context(id)) => {
            assert_eq!(id.origin_name, "Own");
            assert!(id.origin_file.ends_with("own.tsx"), "{:?}", id.origin_file);
        }
        other => panic!("not a proven context: {other:?}"),
    }
}
