//! One run over several projects resolves each file's imports with its own
//! project's aliases.
//!
//! The corpus runs `reactant test-repo` over a directory of repositories, and
//! a monorepo run covers apps and packages that each declare their own
//! tsconfig `paths`. With one resolver per run, taken from the first
//! argument, every `@/…` import of every nested project stayed unresolved, so
//! each such hook was opaque. Each nested project root now scopes the files
//! under it, the innermost one winning.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::{
    driver::{CheckOptions, ReportFormat, run_check},
    resolver::OsFileSystem,
    rules::RuleRegistry,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-project-scopes-{}-{}-{}",
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

    fn arg(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().to_string()
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Run {
    /// Every non-Info finding, as `(rule, component)`.
    defects: Vec<(String, String)>,
    stderr: String,
}

fn run(paths: &[String]) -> Run {
    let opts = CheckOptions {
        info: true,
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
        std::sync::Arc::new(OsFileSystem),
        paths,
        &RuleRegistry::natives(),
        &opts,
        &|p| p.to_string_lossy().to_string(),
    );
    let doc: serde_json::Value =
        serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", out.stdout));
    let defects = doc["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter(|d| d["severity"].as_str() != Some("info"))
        .map(|d| {
            (
                d["rule"].as_str().unwrap_or_default().to_string(),
                d["component"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    Run {
        defects,
        stderr: out.stderr,
    }
}

impl Run {
    fn flags(&self, component: &str) -> bool {
        self.defects.iter().any(|(_, c)| c == component)
    }
}

/// Sets its own state on every render: a defect in whatever component runs
/// this body.
const BUGGY: &str = r#"
import { useState } from "react";

export function useThing() {
  const [n, setN] = useState(0);
  setN(n + 1);
  return n;
}
"#;

const CLEAN: &str = r#"
import { useState } from "react";

export function useThing() {
  const [n] = useState(0);
  return n;
}
"#;

fn app(name: &str) -> String {
    format!(
        r#"
import {{ useThing }} from "@/hooks/useThing";

export function {name}() {{
  const n = useThing();
  return <div>{{n}}</div>;
}}
"#
    )
}

const PATHS: &str = r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#;

/// Two sibling projects, each mapping `@/*` to its own `src/`, each with its
/// own `useThing`; only `buggy`'s hook is the defective one. No marker in
/// the parent, so the parent's own context resolves nothing.
fn two_projects(label: &str, buggy: &str) -> Tmp {
    let tmp = Tmp::new(label);
    for (project, component) in [("a", "AppA"), ("b", "AppB")] {
        tmp.write(&format!("{project}/tsconfig.json"), PATHS);
        tmp.write(&format!("{project}/src/App.tsx"), &app(component));
        let hook = if project == buggy { BUGGY } else { CLEAN };
        tmp.write(&format!("{project}/src/hooks/useThing.ts"), hook);
    }
    tmp
}

/// Run from the parent, each component reaches its own project's hook, and
/// never the other's, whichever of the two sorts first.
#[test]
fn each_nested_project_resolves_its_own_alias() {
    for (buggy, flagged, clean) in [("a", "AppA", "AppB"), ("b", "AppB", "AppA")] {
        let tmp = two_projects(buggy, buggy);
        let found = run(&[tmp.arg("")]);
        assert!(found.flags(flagged), "{flagged}: {:?}", found.defects);
        assert!(!found.flags(clean), "{clean}: {:?}", found.defects);
    }
}

/// Naming the projects one by one gives the same answer as naming their
/// parent: the second argument no longer borrows the first one's resolver.
#[test]
fn several_project_arguments_each_keep_their_own_aliases() {
    let tmp = two_projects("args", "b");
    let found = run(&[tmp.arg("a"), tmp.arg("b")]);
    assert!(found.flags("AppB"), "{:?}", found.defects);
    assert!(!found.flags("AppA"), "{:?}", found.defects);
}

/// A nested project overrides the one around it: the inner tsconfig maps
/// `@/*` elsewhere, and the inner file follows it.
#[test]
fn the_innermost_project_wins() {
    let tmp = Tmp::new("inner");
    tmp.write("tsconfig.json", PATHS);
    tmp.write("src/hooks/useThing.ts", CLEAN);
    tmp.write("packages/inner/tsconfig.json", PATHS);
    tmp.write("packages/inner/src/App.tsx", &app("Inner"));
    tmp.write("packages/inner/src/hooks/useThing.ts", BUGGY);
    let found = run(&[tmp.arg("")]);
    assert!(found.flags("Inner"), "{:?}", found.defects);
}

/// A single project resolves and warns exactly as before: a Vite project
/// without `paths` still says its aliases are unresolved.
#[test]
fn a_single_project_keeps_its_warning() {
    let tmp = Tmp::new("single");
    tmp.write("vite.config.ts", "export default {}");
    tmp.write("src/App.tsx", &app("App"));
    tmp.write("src/hooks/useThing.ts", BUGGY);
    let found = run(&[tmp.arg("")]);
    assert!(
        found.stderr.contains("no tsconfig `paths` found"),
        "{}",
        found.stderr
    );
    assert!(!found.flags("App"), "{:?}", found.defects);
}
