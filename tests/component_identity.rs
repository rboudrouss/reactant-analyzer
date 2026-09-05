//! A JSX callee resolves through the file's imports, not through the bare name
//! (#7).
//!
//! Two files defining `Widget`, one buggy, one clean: the callee's own file
//! settles which one `<Widget/>` means. Resolving from the name alone answered
//! "the file that sorts first", so a decoy component in an unrelated directory
//! silently replaced the real child — and with it every finding that depended
//! on the real child's body. That is a false negative, not a precision loss.

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
            "reactant-identity-{}-{}-{}",
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

/// Every finding, as `(rule, component)` pairs, from a default (`Heuristic`
/// roots) run over the directory — the shape a user gets from `reactant <dir>`.
fn findings(tmp: &Tmp, info: bool) -> Vec<(String, String)> {
    let opts = CheckOptions {
        info,
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
                d["component"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Calls the setter it is handed during render — an Error the parent's own
/// state makes provable, and only if *this* body is the one inlined.
const BUGGY: &str = r#"
export function Widget({ onChange }) {
  onChange(1);
  return <div>buggy</div>;
}
"#;

const CLEAN: &str = r#"
export function Widget() {
  return <div>clean</div>;
}
"#;

fn app_importing(specifier: &str) -> String {
    format!(
        r#"
import {{ useState }} from "react";
import {{ Widget }} from "{specifier}";

export function App() {{
  const [n, setN] = useState(0);
  return <Widget onChange={{setN}} value={{n}} />;
}}
"#
    )
}

/// The import points at the second file alphabetically, so first-match-by-file
/// picks the decoy. The Error must still be reported.
#[test]
fn the_imported_definition_is_the_one_inlined_not_the_first_by_path() {
    let tmp = Tmp::new("import-wins");
    tmp.write("App.tsx", &app_importing("./b/Widget"));
    tmp.write("a/Widget.tsx", CLEAN);
    tmp.write("b/Widget.tsx", BUGGY);

    let found = findings(&tmp, false);
    assert!(
        found
            .iter()
            .any(|(rule, comp)| rule == "cross-setter-in-render" && comp.contains("b/Widget.tsx")),
        "the buggy `b/Widget` is what App renders; got {found:?}"
    );
}

/// The mirror image: the decoy is the one that sorts first *and* is buggy. A
/// resolver that ignored the import would report a finding here that the
/// program cannot produce.
#[test]
fn a_same_named_decoy_that_is_not_imported_is_not_inlined() {
    let tmp = Tmp::new("decoy-quiet");
    tmp.write("App.tsx", &app_importing("./b/Widget"));
    tmp.write("a/Widget.tsx", BUGGY);
    tmp.write("b/Widget.tsx", CLEAN);

    let found = findings(&tmp, false);
    assert!(
        !found
            .iter()
            .any(|(rule, _)| rule == "cross-setter-in-render"),
        "`a/Widget` is never rendered by App, so its setter call is unreachable \
         from this parent; got {found:?}"
    );
}

/// An alias binds a different local name than the origin exports. The origin's
/// name is what the registry is keyed by, so the alias has to be resolved
/// through the import rather than looked up as written.
#[test]
fn an_aliased_import_resolves_to_the_name_the_origin_exports() {
    let tmp = Tmp::new("alias");
    tmp.write(
        "App.tsx",
        r#"
import { useState } from "react";
import { Widget as Panel } from "./b/Widget";

export function App() {
  const [n, setN] = useState(0);
  return <Panel onChange={setN} value={n} />;
}
"#,
    );
    tmp.write("a/Widget.tsx", CLEAN);
    tmp.write("b/Widget.tsx", BUGGY);

    let found = findings(&tmp, false);
    assert!(
        found
            .iter()
            .any(|(rule, comp)| rule == "cross-setter-in-render" && comp.contains("b/Widget.tsx")),
        "`<Panel/>` is `b/Widget`'s `Widget`; got {found:?}"
    );
}

/// A component declared in the same file needs no import to be settled.
#[test]
fn a_locally_declared_child_wins_over_a_same_named_component_elsewhere() {
    let tmp = Tmp::new("local");
    tmp.write(
        "App.tsx",
        format!(
            r#"
import {{ useState }} from "react";

export function App() {{
  const [n, setN] = useState(0);
  return <Widget onChange={{setN}} value={{n}} />;
}}
{BUGGY}
"#
        )
        .as_str(),
    );
    tmp.write("a/Widget.tsx", CLEAN);

    let found = findings(&tmp, false);
    assert!(
        found
            .iter()
            .any(|(rule, comp)| rule == "cross-setter-in-render" && comp.contains("App.tsx")),
        "the `Widget` declared next to `App` is the one it renders; got {found:?}"
    );
}

/// Nothing settles the reference — a namespace import the resolver cannot map
/// to a binding — and two files answer to the name. Picking one is a guess, so
/// the child is treated as unanalysable and the limitation is reported.
#[test]
fn an_unsettled_ambiguous_callee_is_treated_as_unknown_and_reported() {
    let tmp = Tmp::new("ambiguous");
    tmp.write(
        "App.tsx",
        r#"
import { useState } from "react";
import * as UI from "./b/Widget";

export function App() {
  const [n, setN] = useState(0);
  const Widget = UI.Widget;
  return <Widget onChange={setN} value={n} />;
}
"#,
    );
    tmp.write("a/Widget.tsx", CLEAN);
    tmp.write("b/Widget.tsx", BUGGY);

    let found = findings(&tmp, true);
    assert!(
        found.iter().any(|(rule, comp)| rule == "analysis-limit"
            && comp.contains("App")
            && !comp.contains("Widget")),
        "the unresolvable `<Widget/>` must be reported as a limitation, not \
         silently resolved to one of the two; got {found:?}"
    );
}

/// The property #7 was really about: a component's **identity** is independent
/// of the rest of the project, even though its *displayed* name is not.
///
/// Adding an unrelated file that happens to define a second `Widget` renames
/// the first one in the report — that is what the `@file` suffix is for. What
/// must not change is any finding about it. Before ids, the display name was
/// also the key of the results map, the shared-state store and every
/// `Versioned` label, so the unrelated file re-keyed all of them at once.
#[test]
fn an_unrelated_namesake_renames_a_component_without_changing_its_findings() {
    let bare = Tmp::new("identity-bare");
    bare.write("App.tsx", &app_importing("./b/Widget"));
    bare.write("b/Widget.tsx", BUGGY);

    let collided = Tmp::new("identity-collided");
    collided.write("App.tsx", &app_importing("./b/Widget"));
    collided.write("b/Widget.tsx", BUGGY);
    // Neither imported nor rendered by anything above: it exists only to make
    // `Widget` a colliding name.
    collided.write("unrelated/Widget.tsx", CLEAN);

    let rules_of = |found: &[(String, String)]| {
        let mut v: Vec<String> = found.iter().map(|(rule, _)| rule.clone()).collect();
        v.sort();
        v
    };
    let bare_found = findings(&bare, false);
    let collided_found = findings(&collided, false);

    assert_eq!(
        rules_of(&bare_found),
        rules_of(&collided_found),
        "the unrelated namesake changed which findings exist"
    );
    assert!(
        bare_found
            .iter()
            .any(|(rule, comp)| rule == "cross-setter-in-render" && comp == "Widget"),
        "unique name, bare display name; got {bare_found:?}"
    );
    assert!(
        collided_found
            .iter()
            .any(|(rule, comp)| rule == "cross-setter-in-render"
                && comp.starts_with("Widget@")
                && comp.ends_with("b/Widget.tsx")),
        "collided name, qualified display name pointing at the right file; \
         got {collided_found:?}"
    );
}
