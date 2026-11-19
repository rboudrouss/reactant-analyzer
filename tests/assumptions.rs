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
            std::fs::write(dir.join(name), text).unwrap();
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
