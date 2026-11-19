//! A read off a plain object React builds — the props object, the object
//! `useRef` returns — runs no accessor of the program but one a descriptor
//! installed or one of a prototype the program set (#336): a `Proxy`, a
//! literal's or a class's accessor elsewhere in the program leaves it a
//! member read. A read off a prop's or a ref's *value* keeps #334's reading.

use std::process::{Command, Output};

fn reactant(dir: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg(dir)
        .args([
            "--config",
            "tests/fixtures/all-rules.json",
            "--format",
            "json",
            "--fail-on",
            "never",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every finding of the program `src`: `rule:severity message`.
fn findings(src: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "reactant-plain-{}-{}",
        std::process::id(),
        src.len() as u64 * 31 + src.bytes().map(u64::from).sum::<u64>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.tsx"), src).unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            format!(
                "{}:{} {}",
                d["rule"].as_str().unwrap(),
                d["severity"].as_str().unwrap(),
                d["message"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    found
}

fn loops(src: &str) -> Vec<String> {
    findings(src)
        .into_iter()
        .filter(|f| f.starts_with("infinite-loop:"))
        .collect()
}

const PROXY: &str = "export const trap: any = new Proxy({}, { get: () => ({}) });\n";

/// The child reads a prop its parent does not pass (`hide`): a read past
/// the props object's members, of `Object.prototype`. With only a `Proxy`
/// in the program no accessor may be there, so the props object does not
/// escape and `onClickOutside` stays the parent's new arrow.
const ABSENT_PROP: &str = r#"
import { useCallback, useState } from "react";

export const DateInput = ({ onClickOutside, value, hide }: { onClickOutside: (v: number) => void; value: number; hide?: boolean }) => {
  const [v] = useState(value);
  const handle = useCallback(() => {
    onClickOutside(v);
  }, [onClickOutside, v]);
  return <div onClick={handle} hidden={hide} />;
};

export const Field = () => {
  const handleClickOutside = (n: number) => {
    console.log(n);
  };
  return <DateInput onClickOutside={handleClickOutside} value={1} />;
};
"#;

#[test]
fn a_prop_the_parent_does_not_pass_keeps_the_fresh_props_fresh() {
    let found = findings(&format!("{PROXY}{ABSENT_PROP}"));
    assert!(
        found
            .iter()
            .any(|f| f.starts_with("always-unstable-deps:warning")
                && f.contains("`onClickOutside`, a new reference every render")),
        "{found:#?}"
    );
}

/// The looping twin: a getter the program installs on `Object.prototype`
/// under the name of the prop the parent does not pass runs at the child's
/// read, a new object at every render.
#[test]
fn a_getter_installed_on_object_prototype_runs_at_an_absent_prop() {
    let src = r#"
import { useEffect, useState } from "react";

Object.defineProperty(Object.prototype, "hide", {
  get() {
    return {};
  },
  configurable: true,
});

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
    assert_eq!(loops(src).len(), 1, "{:#?}", findings(src));
    // Under another name, the read runs nothing.
    assert!(loops(&src.replace("\"hide\"", "\"other\"")).is_empty());
}

/// An effect over `dep`, read off a ref started at `init` and set up by
/// `setup`, with a `Proxy` elsewhere. A member past `current` is read off a
/// ref started at `null`: one started at an object may hold a member a
/// write moves.
fn ref_effect(init: &str, setup: &str, dep: &str) -> String {
    format!(
        r#"
import {{ useEffect, useRef, useState }} from "react";

const trap = new Proxy({{}}, {{ get: () => ({{}}) }});

export function App() {{
  const [n, setN] = useState(0);
  const ref: any = useRef({init});
  {setup}
  useEffect(() => {{
    setN((c) => c + 1);
  }}, [{dep}]);
  return <div>{{n}}</div>;
}}
"#
    )
}

#[test]
fn a_read_off_a_ref_with_a_proxy_elsewhere_holds_still() {
    assert!(loops(&ref_effect("{ k: 1 }", "", "ref.current")).is_empty());
    assert!(loops(&ref_effect("null", "", "ref.missing")).is_empty());
}

#[test]
fn an_accessor_defined_on_the_ref_or_its_prototype_runs() {
    for (init, setup, dep) in [
        (
            "{ k: 1 }",
            "Object.defineProperty(ref, \"current\", { get: () => ({}), configurable: true });",
            "ref.current",
        ),
        ("null", "Object.setPrototypeOf(ref, trap);", "ref.missing"),
        ("null", "ref.__proto__ = trap;", "ref.missing"),
    ] {
        let src = ref_effect(init, setup, dep);
        assert_eq!(loops(&src).len(), 1, "{setup}: {:#?}", findings(&src));
    }
}

/// The prop's value is not React's: a read off it keeps #334's reading.
#[test]
fn a_read_off_a_props_value_stays_a_possible_accessor_call() {
    let src = format!(
        r#"{PROXY}
import {{ useEffect, useState }} from "react";

function C({{ activeTool }}: {{ activeTool: {{ type: string }} }}) {{
  const [n, setN] = useState(0);
  useEffect(() => {{
    setN((c) => c + 1);
  }}, [activeTool.type]);
  return <div>{{n}}</div>;
}}

export function App() {{
  const [tool, setTool] = useState({{ type: "a" }});
  return <div onClick={{() => setTool({{ type: "b" }})}}><C activeTool={{tool}} /></div>;
}}
"#
    );
    let found = loops(&src);
    assert!(
        found
            .iter()
            .any(|f| f.contains("`activeTool.type`, which may differ at any render")),
        "{found:#?}"
    );
}

/// The pair `useState` returns: its elements are the slot and its setter.
#[test]
fn the_state_pair_is_read_as_its_slot_and_its_setter() {
    let src = format!(
        r#"{PROXY}
import {{ useEffect, useState }} from "react";

export function App() {{
  const [n, setN] = useState(0);
  const [s, setS] = useState("a");
  const pair = useState("b");
  useEffect(() => {{
    setN((c) => c + 1);
  }}, [s, setS, pair[0], pair[1]]);
  return <div onClick={{() => setS("b")}}>{{n}}</div>;
}}
"#
    );
    assert!(loops(&src).is_empty(), "{:#?}", findings(&src));
}
