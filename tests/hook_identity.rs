//! A custom hook call resolves through the call's own import, never through
//! a same-named hook elsewhere in the walked tree (#226).
//!
//! The decoy is the hook that sorts first by path. Before #226, an import the
//! resolver could not map (`@pkg/x`) — or a resolved barrel that only
//! re-exports the hook — fell back to that first homonym and inlined its body
//! in place of ⊤: a specific wrong behaviour, which can hide a real loop as
//! well as invent one. ADR-013 accepts the name fallback only for a call
//! with no import at all.

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
            "reactant-hook-identity-{}-{}-{}",
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

/// Every finding, as `(rule, severity, component)`, from a default run.
fn findings(tmp: &Tmp) -> Vec<(String, String, String)> {
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
        .map(|d| {
            (
                d["rule"].as_str().unwrap_or_default().to_string(),
                d["severity"].as_str().unwrap_or_default().to_string(),
                d["component"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Sets its own state on every render: an Error in whatever component runs
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

fn app(import: &str) -> String {
    format!(
        r#"
{import}

export function App() {{
  const n = useThing();
  return <div>{{n}}</div>;
}}
"#
    )
}

/// A non-Info finding on `App`.
fn app_defect(found: &[(String, String, String)]) -> bool {
    found
        .iter()
        .any(|(_, sev, comp)| comp == "App" && sev != "info")
}

/// The issue's case: `useThing` comes from a package the resolver cannot map;
/// the walked tree's `useThing` is not it. Opaque, Info at most.
#[test]
fn an_unresolved_package_import_does_not_run_a_homonym_s_body() {
    let tmp = Tmp::new("pkg");
    tmp.write("App.tsx", &app(r#"import { useThing } from "@pkg/x";"#));
    tmp.write("a/useThing.ts", BUGGY);
    tmp.write("b/useThing.ts", CLEAN);

    let found = findings(&tmp);
    assert!(
        !app_defect(&found),
        "`@pkg/x`'s `useThing` is not `a/useThing.ts`; got {found:?}"
    );
}

/// A resolved barrel that only re-exports the hook leads to the file it
/// names, not to the first homonym by path.
#[test]
fn a_barrel_import_reaches_the_re_exported_definition() {
    for barrel in [
        r#"export * from "./useThing";"#,
        r#"export { useThing } from "./useThing";"#,
        "import { useThing } from \"./useThing\";\nexport { useThing };",
    ] {
        let tmp = Tmp::new("barrel");
        tmp.write("App.tsx", &app(r#"import { useThing } from "./lib";"#));
        tmp.write("a/useThing.ts", CLEAN);
        tmp.write("lib/index.ts", barrel);
        tmp.write("lib/useThing.ts", BUGGY);

        let found = findings(&tmp);
        assert!(
            app_defect(&found),
            "`./lib` re-exports `lib/useThing.ts` (`{barrel}`); got {found:?}"
        );
    }
}

/// A default import of a barrel that forwards its default
/// (`export { default } from "./useThing"`) reaches that file's default
/// export.
#[test]
fn a_default_import_reaches_a_forwarded_default() {
    let tmp = Tmp::new("default-barrel");
    tmp.write("App.tsx", &app(r#"import useThing from "./lib";"#));
    tmp.write("a/useThing.ts", CLEAN);
    tmp.write("lib/index.ts", r#"export { default } from "./useThing";"#);
    tmp.write(
        "lib/useThing.ts",
        &BUGGY.replace("export function", "export default function"),
    );

    let found = findings(&tmp);
    assert!(
        app_defect(&found),
        "`./lib` forwards the default of `lib/useThing.ts`; got {found:?}"
    );
}

/// The mirror image: the barrel's target is clean and the first homonym by
/// path is buggy. Following the name instead of the chain reports a defect the
/// program cannot have.
#[test]
fn a_barrel_import_does_not_reach_a_homonym_off_its_chain() {
    let tmp = Tmp::new("barrel-decoy");
    tmp.write("App.tsx", &app(r#"import { useThing } from "./lib";"#));
    tmp.write("a/useThing.ts", BUGGY);
    tmp.write("lib/index.ts", r#"export * from "./useThing";"#);
    tmp.write("lib/useThing.ts", CLEAN);

    let found = findings(&tmp);
    assert!(
        !app_defect(&found),
        "`./lib` re-exports the clean `lib/useThing.ts`; got {found:?}"
    );
}

/// ADR-013: a call with no import falls back to the name, first match by
/// path included.
#[test]
fn an_unimported_name_still_falls_back_to_the_first_homonym() {
    let tmp = Tmp::new("no-import");
    tmp.write("App.tsx", &app(""));
    tmp.write("a/useThing.ts", BUGGY);
    tmp.write("b/useThing.ts", CLEAN);

    let found = findings(&tmp);
    assert!(
        app_defect(&found),
        "no import: the name settles on `a/useThing.ts`; got {found:?}"
    );
}

/// A hook declared next to the component is the one it calls, whatever else
/// the tree defines under the name.
#[test]
fn a_same_file_hook_wins_over_a_homonym_elsewhere() {
    let tmp = Tmp::new("same-file");
    tmp.write(
        "App.tsx",
        &format!("{}\n{}", app(r#"import { useState } from "react";"#), {
            BUGGY.replace("import { useState } from \"react\";", "")
        }),
    );
    tmp.write("a/useThing.ts", CLEAN);

    let found = findings(&tmp);
    assert!(
        app_defect(&found),
        "the `useThing` declared in App.tsx is the one App runs; got {found:?}"
    );
}
