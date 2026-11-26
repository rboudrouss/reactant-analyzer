//! Assumptions are configuration (ADR-048): off by default, declared in the
//! config's `assumptions` section or with `--assume`, stated once in every
//! output format. `plainObjectsRunNoAccessor` takes the accessor the
//! analysis cannot place off the table (#336): it loses the loop through a
//! getter installed on `Object.prototype`, and recovers the findings a
//! descriptor or a prototype set on an unrelated object (a polyfill's
//! variable-named getter) hides. Both sides are pinned here.

use std::process::{Command, Output};

const NAME: &str = "plainObjectsRunNoAccessor";

/// A getter on `Object.prototype` under the name of a prop the parent does
/// not pass: React runs it at the child's read (oracle case
/// `plain-absent-prop-object-prototype-getter-loops`, `react=loop`).
const OBJECT_PROTOTYPE_GETTER: &str = "plain-absent-prop-object-prototype-getter-loops";

/// The two oracle loops through an accessor on an object the analysis
/// places (the ref itself, or one it hands to `Object.setPrototypeOf`): the
/// assumption does not touch them, the object is not a plain one the library
/// builds.
const PLACED_ACCESSOR_LOOPS: [&str; 2] = [
    "plain-ref-defined-accessor-loops",
    "plain-ref-prototype-proxy-loops",
];

/// What the assumption recovers: the program installs a getter, or sets a
/// prototype, on an object unrelated to the ref the effect reads. React
/// converges; without the premise the analysis cannot place the target.
const UNRELATED_TARGETS: [(&str, &str); 2] = [
    (
        "setPrototypeOf on another object",
        r#"import { useEffect, useRef, useState } from "react";
const trap = new Proxy({}, { get: () => ({}) });
const other: any = {};
Object.setPrototypeOf(other, trap);
export function App() {
  const [n, setN] = useState(0);
  const ref: any = useRef(null);
  useEffect(() => {
    setN((c) => c + 1);
  }, [ref.missing]);
  return <div>{n}</div>;
}
"#,
    ),
    (
        "defineProperty on another object",
        r#"import { useEffect, useRef, useState } from "react";
const t: any = {};
Object.defineProperty(t, "current", { get: () => ({}), configurable: true });
export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<any>({});
  useEffect(() => {
    setN((c) => c + 1);
  }, [ref.current]);
  return <div>{n}</div>;
}
"#,
    ),
];

fn reactant(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg("check")
        .arg(dir)
        .args(["--fail-on", "never"])
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A directory holding `files`, removed when dropped.
struct Project(std::path::PathBuf);

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("reactant-assume-{}-{run}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            let file = dir.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        Project(dir)
    }

    fn case(case: &str) -> Self {
        Self::case_with_config(case, None)
    }

    /// The case, with a `reactant.config.json` at the root (discovered there)
    /// when `config` is given.
    fn case_with_config(case: &str, config: Option<&str>) -> Self {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("oracle/cases/{case}.tsx")),
        )
        .unwrap();
        let mut files = vec![("a.tsx", src.as_str())];
        files.extend(config.map(|c| ("reactant.config.json", c)));
        Self::new(&files)
    }

    fn run(&self, args: &[&str]) -> Output {
        reactant(&self.0, args)
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut all = vec!["--format", "json"];
        all.extend_from_slice(args);
        let out = self.run(&all);
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// The loop findings (`rule:severity`) of a JSON report.
fn loops(doc: &serde_json::Value) -> Vec<String> {
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"].as_str() != Some("info"))
        .filter(|d| d["rule"].as_str().unwrap().contains("loop"))
        .map(|d| {
            format!(
                "{}:{}",
                d["rule"].as_str().unwrap(),
                d["severity"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

fn assumptions(doc: &serde_json::Value) -> Vec<String> {
    doc["assumptions"]
        .as_array()
        .expect("`assumptions` is always an array")
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

/// The documented trade-off, the cost side. Off: the loop through the
/// `Object.prototype` getter is reported and nothing is declared. On: the
/// premise says there is none, the loop is not reported, and the output says
/// so.
#[test]
fn the_assumption_loses_the_object_prototype_getter_loop_and_says_so() {
    let p = Project::case(OBJECT_PROTOTYPE_GETTER);
    let off = p.json(&[]);
    assert_eq!(loops(&off), ["infinite-loop:warning"], "off");
    assert!(assumptions(&off).is_empty(), "off");

    let on = p.json(&["--assume", NAME]);
    assert!(loops(&on).is_empty(), "on, {:?}", loops(&on));
    assert_eq!(assumptions(&on), [NAME], "on");
}

/// A loop through an accessor on an object the analysis places is found
/// whatever the run declares.
#[test]
fn the_assumption_keeps_the_loops_through_a_placed_accessor() {
    for case in PLACED_ACCESSOR_LOOPS {
        let p = Project::case(case);
        assert_eq!(
            loops(&p.json(&[])),
            ["infinite-loop:warning"],
            "{case}: off"
        );
        let on = p.json(&["--assume", NAME]);
        assert_eq!(loops(&on), ["infinite-loop:warning"], "{case}: on");
        assert_eq!(assumptions(&on), [NAME], "{case}: on");
    }
}

/// The benefit side. Off: the analysis cannot place the target of the
/// descriptor or of the prototype, so any read of an absent member may run
/// it (a sound false positive). On: it is recovered.
#[test]
fn the_assumption_recovers_what_an_unrelated_accessor_hides() {
    for (name, src) in UNRELATED_TARGETS {
        let p = Project::new(&[("a.tsx", src)]);
        assert_eq!(
            loops(&p.json(&[])),
            ["infinite-loop:warning"],
            "{name}: off"
        );
        assert!(loops(&p.json(&["--assume", NAME])).is_empty(), "{name}: on");
    }
}

/// A loop no accessor is needed for stays reported under the assumption.
#[test]
fn the_assumption_keeps_a_loop_no_accessor_makes() {
    let p = Project::case("plain-member-allocated-dep-loops");
    let off = p.json(&[]);
    assert!(!loops(&off).is_empty(), "the case loops");
    assert_eq!(loops(&p.json(&["--assume", NAME])), loops(&off));
}

#[test]
fn the_config_section_declares_it_and_the_flag_adds_to_it() {
    let declared = format!(r#"{{ "assumptions": {{ "{NAME}": true }} }}"#);
    let p = Project::case_with_config(OBJECT_PROTOTYPE_GETTER, Some(&declared));
    let doc = p.json(&[]);
    assert_eq!(assumptions(&doc), [NAME]);
    assert!(loops(&doc).is_empty());

    // Explicitly off in the config, turned on by the flag.
    let off = format!(r#"{{ "assumptions": {{ "{NAME}": false }} }}"#);
    let p = Project::case_with_config(OBJECT_PROTOTYPE_GETTER, Some(&off));
    assert!(assumptions(&p.json(&[])).is_empty());
    assert_eq!(assumptions(&p.json(&["--assume", NAME])), [NAME]);
}

#[test]
fn a_misspelt_name_is_an_error_never_no_premise() {
    let p = Project::case("plain-ref-defined-accessor-loops");
    let out = p.run(&["--assume", "plainObjectRunNoAccessor"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown assumption"), "{err}");
    assert!(err.contains(NAME), "{err}");

    let p = Project::case_with_config(
        OBJECT_PROTOTYPE_GETTER,
        Some(r#"{ "assumptions": { "plainObjectRunNoAccessor": true } }"#),
    );
    let out = p.run(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("plainObjectRunNoAccessor"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_human_output_states_it_once_and_only_when_declared() {
    let p = Project::case("plain-ref-defined-accessor-loops");
    let line = "1 assumption in force: plain objects run no accessor of the program";
    let off = String::from_utf8(p.run(&[]).stdout).unwrap();
    assert!(!off.contains("assumption"), "{off}");
    let on = String::from_utf8(p.run(&["--assume", NAME]).stdout).unwrap();
    assert_eq!(on.matches(line).count(), 1, "{on}");
    // Declared twice, stated once.
    let twice = String::from_utf8(p.run(&["--assume", NAME, "--assume", NAME]).stdout).unwrap();
    assert_eq!(twice.matches(line).count(), 1, "{twice}");
}

#[test]
fn the_config_schema_holds_the_section() {
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/schemas/reactant-config.schema.json"),
    )
    .unwrap();
    let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();
    let section = &schema["$defs"]["Assumptions"];
    assert!(section["properties"][NAME].is_object(), "{schema:#}");
    assert_eq!(section["additionalProperties"], false);
}

// ── foreignRealmFiles (#379) ─────────────────────────────────────────────────

const FOREIGN: &str = "foreignRealmFiles";

/// A polyfill installing a getter under a name no constant spells, on
/// `Object.prototype`: the program-wide accessor fact becomes "any name".
const POLYFILL: &str = r#"const names = ["hide", "other"];
for (const name of names) {
  Object.defineProperty(Object.prototype, name, {
    get() {
      return {};
    },
    configurable: true,
  });
}
"#;

/// A child reading a prop its parent does not pass, in an effect's deps: it
/// loops only if some getter of the program runs at the read.
const APP: &str = r#"import { useEffect, useState } from "react";

function C({ hide }: { hide?: object }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [hide]);
  return <div>{n}</div>;
}

export function App() {
  return <C />;
}
"#;

fn values(doc: &serde_json::Value) -> Option<Vec<String>> {
    doc.get("assumption_values")?[FOREIGN]
        .as_array()
        .map(|globs| {
            globs
                .iter()
                .map(|g| g.as_str().unwrap().to_string())
                .collect()
        })
}

/// The documented trade-off, pinned on both sides. By default the polyfill's
/// getter may run at the app's read, so the loop is reported; with the file
/// declared foreign the app no longer reads it, nothing is reported, and the
/// output says what was assumed.
#[test]
fn a_foreign_polyfill_feeds_no_fact_the_app_reads() {
    let p = Project::new(&[("src/polyfill.ts", POLYFILL), ("src/App.tsx", APP)]);
    let off = p.json(&[]);
    assert_eq!(loops(&off), ["infinite-loop:warning"], "off");
    assert!(assumptions(&off).is_empty(), "off");
    assert!(
        off.get("assumption_values").is_none(),
        "off: byte-identical"
    );

    let on = p.json(&["--assume", "foreignRealmFiles=src/polyfill.ts"]);
    assert!(loops(&on).is_empty(), "on, {:?}", loops(&on));
    assert_eq!(assumptions(&on), [FOREIGN], "on");
    assert_eq!(values(&on), Some(vec!["src/polyfill.ts".to_string()]));
}

/// #389: an app imports `Trans` from a package while a declared file defines
/// a `Trans` of its own. The app's element is the package's component, which
/// the analysis does not see — never the declared homonym, whose analysis
/// reads the whole program's fact and whose heap, joined back into its
/// parents', would make every member read of the app a hand-over (S34's
/// probe4: `Child` reported under the polyfill's "any name" fact).
#[test]
fn a_package_homonym_in_a_declared_file_feeds_no_fact_to_the_app() {
    const APP: &str = r#"import { useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { useHot } from './useHot';
function Label() {
  return <Trans>hello</Trans>;
}
function Child({ setOpen }: { setOpen: (b: boolean) => void }) {
  useHot({ keys: 'esc', callback: () => setOpen(false) });
  return <div onClick={() => setOpen(true)}><Label /></div>;
}
export function Parent() {
  const [open, setOpen] = useState(false);
  return <>{String(open)}<Child setOpen={setOpen} /></>;
}
"#;
    const USE_HOT: &str = r#"export const useHot = ({ keys, callback, options }: { keys: string; callback: () => void; options?: { a?: boolean } }) => {
  const a = options?.a ? 1 : 0;
  return [keys.length + a, callback];
};
"#;
    const INSTALL: &str = r#"export function install(target: object, name: string) {
  Object.defineProperty(target, name, { get() { return 1; } });
}
"#;
    const SDK_TRANS: &str = r#"export const Trans = ({ children }: { children?: string }) => <span>{children}</span>;
"#;
    let p = Project::new(&[
        ("app/App.tsx", APP),
        ("app/useHot.ts", USE_HOT),
        ("poly/install.ts", INSTALL),
        ("sdk/Trans.tsx", SDK_TRANS),
    ]);
    let handed = |doc: &serde_json::Value| -> Vec<String> {
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["rule"].as_str() == Some("cross-setter-in-render"))
            .map(|d| d["message"].as_str().unwrap().to_string())
            .collect()
    };
    // The hand-over row is an Info: both runs list them.
    let off = p.json(&["--info"]);
    assert_eq!(handed(&off).len(), 1, "off: the polyfill's any-name fact");
    let on = p.json(&[
        "--info",
        "--assume",
        "foreignRealmFiles=poly/**",
        "--assume",
        "foreignRealmFiles=sdk/**",
    ]);
    assert!(handed(&on).is_empty(), "on: {:?}", handed(&on));
}

/// A glob that names no file declares a premise that changes nothing: the
/// loop stays, and the output still states it.
#[test]
fn a_glob_matching_no_file_changes_nothing_and_is_stated() {
    let p = Project::new(&[("src/polyfill.ts", POLYFILL), ("src/App.tsx", APP)]);
    let on = p.json(&["--assume", "foreignRealmFiles=src/elsewhere/**"]);
    assert_eq!(loops(&on), ["infinite-loop:warning"]);
    assert_eq!(assumptions(&on), [FOREIGN]);
}

/// Globs are read from the project root, the config's list and the flags add
/// up, and the human output prints them once.
#[test]
fn the_globs_come_from_the_config_and_the_flag_and_are_stated_once() {
    let config = r#"{ "assumptions": { "foreignRealmFiles": ["**/polyfills/**"] } }"#;
    let p = Project::new(&[
        ("src/polyfills/install.ts", POLYFILL),
        ("src/App.tsx", APP),
        ("reactant.config.json", config),
    ]);
    let doc = p.json(&["--assume", "foreignRealmFiles=apps/server/**"]);
    assert!(loops(&doc).is_empty(), "{:?}", loops(&doc));
    assert_eq!(
        values(&doc),
        Some(vec![
            "apps/server/**".to_string(),
            "**/polyfills/**".to_string()
        ])
    );
    let human = String::from_utf8(
        p.run(&["--assume", "foreignRealmFiles=**/polyfills/**"])
            .stdout,
    )
    .unwrap();
    let line =
        "1 assumption in force: files matching **/polyfills/** do not share a realm with the app";
    assert_eq!(human.matches(line).count(), 1, "{human}");
}

/// An empty list declares nothing.
#[test]
fn an_empty_list_is_no_assumption() {
    let p = Project::new(&[
        ("src/polyfill.ts", POLYFILL),
        ("src/App.tsx", APP),
        (
            "reactant.config.json",
            r#"{ "assumptions": { "foreignRealmFiles": [] } }"#,
        ),
    ]);
    let doc = p.json(&[]);
    assert_eq!(loops(&doc), ["infinite-loop:warning"]);
    assert!(assumptions(&doc).is_empty());
    assert!(doc.get("assumption_values").is_none());
}

/// The inverse: a declared file is still analysed and reported, and its own
/// reads still see what it installs. The getter on `Object.prototype` and
/// the component that reads it share the declared file, so the loop is
/// reported; so is a loop through an accessor on a placed object.
#[test]
fn a_declared_file_still_reads_its_own_accessors() {
    for case in [OBJECT_PROTOTYPE_GETTER, PLACED_ACCESSOR_LOOPS[0]] {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("oracle/cases/{case}.tsx")),
        )
        .unwrap();
        let p = Project::new(&[("src/own/Own.tsx", &src), ("src/Other.tsx", APP)]);
        let on = p.json(&["--assume", "foreignRealmFiles=src/own/**"]);
        let own: Vec<_> = on["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["rule"].as_str().unwrap().contains("loop"))
            .filter(|d| d["file"].as_str().unwrap().contains("Own.tsx"))
            .collect();
        assert_eq!(own.len(), 1, "{case}: {own:?}");
        assert_eq!(assumptions(&on), [FOREIGN], "{case}");
    }
}

#[test]
fn a_malformed_declaration_is_an_error_never_no_premise() {
    let p = Project::case("plain-ref-defined-accessor-loops");
    for bad in [
        "foreignRealmFiles",
        "foreignRealmFiles=",
        "plainObjectsRunNoAccessor=1",
    ] {
        let out = p.run(&["--assume", bad]);
        assert_eq!(out.status.code(), Some(2), "{bad}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("--assume"),
            "{bad}"
        );
    }
    let p = Project::case_with_config(
        "plain-ref-defined-accessor-loops",
        Some(r#"{ "assumptions": { "foreignRealmFiles": "src/**" } }"#),
    );
    assert_eq!(p.run(&[]).status.code(), Some(2));
}

#[test]
fn the_config_schema_holds_the_list_of_globs() {
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/schemas/reactant-config.schema.json"),
    )
    .unwrap();
    let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();
    let list = &schema["$defs"]["Assumptions"]["properties"][FOREIGN];
    assert_eq!(list["type"], "array", "{schema:#}");
    assert_eq!(list["items"]["type"], "string");
}

// ── packagePropsRunOnEvents ──────────────────────────────────────────────────

const ON_EVENTS: &str = "packagePropsRunOnEvents";

/// The hand-over rows (`rule:severity`) of a JSON report run with `--info`.
fn hand_overs(doc: &serde_json::Value) -> Vec<String> {
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"].as_str().unwrap().ends_with("setter-in-render"))
        .map(|d| {
            format!(
                "{}:{}",
                d["rule"].as_str().unwrap(),
                d["severity"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    found
}

fn hidden_hand_overs(doc: &serde_json::Value) -> u64 {
    doc["summary"]["hidden_hand_overs"].as_u64().unwrap()
}

/// A package component calling its prop on a click: by default an Info row
/// (hidden, counted in the summary); declared, the prop is an event's, no row
/// at all and nothing counted as hidden, and the output says what it assumed.
#[test]
fn package_props_on_events_drop_the_hand_over_and_say_so() {
    let p = Project::case("handover-info-package-child-handler-settles");
    let off = p.json(&["--info"]);
    assert_eq!(hand_overs(&off), ["setter-in-render:info"], "off");
    assert!(assumptions(&off).is_empty(), "off");
    assert_eq!(hidden_hand_overs(&p.json(&[])), 1, "off");

    let on = p.json(&["--info", "--assume", ON_EVENTS]);
    assert!(hand_overs(&on).is_empty(), "on: {:?}", hand_overs(&on));
    assert_eq!(assumptions(&on), [ON_EVENTS], "on");
    assert_eq!(
        hidden_hand_overs(&p.json(&["--assume", ON_EVENTS])),
        0,
        "on"
    );
}

/// The price: a package that calls the prop from an effect loops in React;
/// by default the Info row discloses it, declared the analysis says nothing.
#[test]
fn package_props_on_events_lose_the_package_effect_loop() {
    for case in [
        "handover-info-package-child-effect-loops",
        "package-props-named-closure-effect-loops",
    ] {
        let p = Project::case(case);
        assert_eq!(
            hand_overs(&p.json(&["--info"])),
            ["setter-in-render:info"],
            "{case}: off"
        );
        let on = p.json(&["--info", "--assume", ON_EVENTS]);
        assert!(hand_overs(&on).is_empty(), "{case}: on");
        assert!(loops(&on).is_empty(), "{case}: on");
    }
}

/// What the statement does not cover keeps its default reading: a `ref`, the
/// `children`, a spread, a function held by a member of an object prop
/// (written in place or named) and an element of an array prop.
#[test]
fn package_props_on_events_leave_ref_children_spreads_and_members() {
    let shapes = [
        ("ref", r#"<RefTarget ref={() => setN(n + 1)} />"#),
        (
            "children",
            r#"<RenderProp>{() => { setN(n + 1); return null; }}</RenderProp>"#,
        ),
        (
            "children prop",
            r#"<RenderProp children={() => setN(n + 1)} />"#,
        ),
        (
            "spread",
            r#"<EffectCalls {...{ fn: () => setN(n + 1) }} />"#,
        ),
        (
            "object member",
            r#"<EffectCalls opts={{ cb: () => setN(n + 1) }} />"#,
        ),
        (
            "array entry",
            r#"<EffectCalls fns={[() => setN(n + 1)]} />"#,
        ),
        ("named object", r#"<EffectCalls opts={held} />"#),
    ];
    for (name, element) in shapes {
        let src = format!(
            r#"import {{ useState }} from "react";
import {{ EffectCalls, RefTarget, RenderProp }} from "ext";
export function App() {{
  const [n, setN] = useState(0);
  const held = {{ cb: () => setN(n + 1) }};
  return <div>{{n}}{element}</div>;
}}
"#
        );
        let p = Project::new(&[("a.tsx", &src)]);
        let off = hand_overs(&p.json(&["--info"]));
        assert_eq!(off, ["setter-in-render:info"], "{name}: off");
        let on = p.json(&["--info", "--assume", ON_EVENTS]);
        assert_eq!(hand_overs(&on), off, "{name}: on");
        assert_eq!(assumptions(&on), [ON_EVENTS], "{name}: on");
    }
}

/// Code that is not provably a package's keeps its Warning either way: a
/// program child the analysis does not resolve, and a package element once
/// the program writes code into a package import (#401's voiding fact).
#[test]
fn package_props_on_events_leave_program_code_and_a_replaced_package() {
    let p = Project::case("package-props-unresolved-program-child-loops");
    assert_eq!(
        hand_overs(&p.json(&["--info"])),
        ["setter-in-render:warning"]
    );
    assert_eq!(
        hand_overs(&p.json(&["--info", "--assume", ON_EVENTS])),
        ["setter-in-render:warning"]
    );

    let voided = r#"import { useState } from "react";
import * as Ext from "ext";
(Ext as any).hook = (f: () => void) => f();
export function App() {
  const [n, setN] = useState(0);
  return <div>{n}<Ext.ClickCalls fn={() => setN(n + 1)} /></div>;
}
"#;
    let p = Project::new(&[("a.tsx", voided)]);
    assert_eq!(
        hand_overs(&p.json(&["--info"])),
        ["setter-in-render:warning"]
    );
    assert_eq!(
        hand_overs(&p.json(&["--info", "--assume", ON_EVENTS])),
        ["setter-in-render:warning"]
    );
}

/// Declared, a package's prop is a listener a dispatch of any event may
/// reach (#407): the program's own click of the package's button, in an
/// effect after every commit, is a loop the analysis now claims.
#[test]
fn package_props_on_events_are_listeners_a_dispatch_runs() {
    let p = Project::case("package-props-dispatched-click-loops");
    assert!(loops(&p.json(&[])).is_empty(), "off");
    assert_eq!(
        loops(&p.json(&["--assume", ON_EVENTS])),
        ["infinite-loop:warning"],
        "on"
    );
}

/// The loop rules read the same timing: W6's store cycle through a
/// navigating closure handed to a package leaves under the assumption — on
/// the program that settles, and on its neighbour that loops (the price).
#[test]
fn package_props_on_events_remove_the_store_step_from_the_loop_rules() {
    for case in [
        "package-props-store-shared-button-settles",
        "package-props-store-shared-button-loops",
    ] {
        let p = Project::case(case);
        assert_eq!(
            loops(&p.json(&[])),
            ["cross-component-infinite-loop:warning"],
            "{case}: off"
        );
        assert!(
            loops(&p.json(&["--assume", ON_EVENTS])).is_empty(),
            "{case}: on"
        );
    }
}

#[test]
fn package_props_on_events_are_declared_in_the_config_and_stated_in_the_human_output() {
    let declared = format!(r#"{{ "assumptions": {{ "{ON_EVENTS}": true }} }}"#);
    let p = Project::case_with_config(
        "handover-info-package-child-handler-settles",
        Some(&declared),
    );
    let doc = p.json(&["--info"]);
    assert_eq!(assumptions(&doc), [ON_EVENTS]);
    assert!(hand_overs(&doc).is_empty());
    let line = "1 assumption in force: a package component calls its function props only from an event handler";
    let human = String::from_utf8(p.run(&[]).stdout).unwrap();
    assert_eq!(human.matches(line).count(), 1, "{human}");

    // Explicitly off in the config: the default reading, turned on by the flag.
    let off = format!(r#"{{ "assumptions": {{ "{ON_EVENTS}": false }} }}"#);
    let p = Project::case_with_config("handover-info-package-child-handler-settles", Some(&off));
    let doc = p.json(&["--info"]);
    assert!(assumptions(&doc).is_empty());
    assert_eq!(hand_overs(&doc), ["setter-in-render:info"]);
    let human = String::from_utf8(p.run(&[]).stdout).unwrap();
    assert!(!human.contains("assumption"), "{human}");
    assert_eq!(assumptions(&p.json(&["--assume", ON_EVENTS])), [ON_EVENTS]);

    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/schemas/reactant-config.schema.json"),
    )
    .unwrap();
    let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();
    let flag = &schema["$defs"]["Assumptions"]["properties"][ON_EVENTS];
    assert_eq!(flag["type"], "boolean", "{schema:#}");
    assert_eq!(flag["default"], false);
}
