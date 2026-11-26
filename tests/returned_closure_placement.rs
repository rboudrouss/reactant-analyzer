//! A call of a local function the fixpoint ran evaluates to what its bodies
//! returned in every position (#333), not only where a `let` binds it: a
//! member of a literal, an element, a spread source, an argument, a callee,
//! a prop. A closure it returns is placed there, or escapes with the value,
//! as design-walk §3's Invariant I asks ("a return of a body the fixpoint ran
//! keeps it").

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
            "--info",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// One directory per call: two tests of this binary run at the same time,
/// and a directory keyed by the program's text alone is removed under the
/// other test's analyzer (an empty stdout, "EOF while parsing a value").
static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The loop findings (`rule:severity`) of the program `src`, and its Info
/// hand-over rows (a closure handed to code the analysis does not see).
fn findings_of(src: &str, tag: &str) -> Vec<String> {
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reactant-ret-{}-{run}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.tsx"), src).unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| {
            let rule = d["rule"].as_str().unwrap_or("");
            (rule.ends_with("setter-in-render") || rule == "infinite-loop")
                && (d["severity"] != "info" || d["setters"].is_array())
        })
        .map(|d| {
            format!(
                "{}:{}",
                d["rule"].as_str().unwrap(),
                d["severity"].as_str().unwrap()
            )
        })
        .collect()
}

/// The loop findings of `App`, whose render (`effect` false) or effect on
/// `[n]` runs `body`; `mk` returns a closure writing `n + 1`, `mk0` one
/// writing the constant 1.
fn loops(body: &str, effect: bool) -> Vec<String> {
    let run = if effect {
        format!("useEffect(() => {{ {body} }}, [n]);")
    } else {
        body.to_string()
    };
    let src = format!(
        "import {{ useEffect, useState }} from 'react';\n\
         import {{ someCall }} from 'lib';\n\
         export function App() {{\n\
           const [n, setN] = useState(0);\n\
           const mk = () => () => setN(n + 1);\n\
           const mk0 = () => () => setN(1);\n\
           {run}\n\
           return <div>{{n}}{{typeof mk0}}</div>;\n\
         }}\n"
    );
    findings_of(&src, if effect { "e" } else { "r" })
}

/// React loops on each: the closure the call returns is called, or handed
/// to code that may call it, at every pass.
#[test]
fn a_returned_closure_is_placed_in_every_position() {
    for body in [
        // A member of a literal, an element, a spread source.
        "const o = { f: mk() }; o.f();",
        "const xs = [mk()]; xs[0]();",
        "const xs = [mk()]; xs.forEach((f) => f());",
        "const o = { ...{ f: mk() } }; o.f();",
        // An argument: of unseen code, of a local function.
        "someCall(mk());",
        "const run = (f: () => void) => f(); run(mk());",
        // A callee, a member of a call, a literal callee.
        "[mk()][0]();",
        "({ f: mk() }).f();",
        "const wrap = () => ({ f: mk() }); wrap().f();",
    ] {
        for effect in [false, true] {
            assert!(
                !loops(body, effect).is_empty(),
                "{body} (effect: {effect}): silent"
            );
        }
    }
}

/// The whole proof holds where the call has one value: the member is surely
/// the one closure, run at every render.
#[test]
fn a_returned_closure_called_through_a_member_is_a_certain_write() {
    assert_eq!(
        loops("const o = { f: mk() }; o.f();", false),
        ["setter-in-render:error"]
    );
}

/// The neighbours React settles: the closure is kept and never called, or
/// the closure writes a constant from an effect, which bails out.
#[test]
fn a_returned_closure_that_is_not_called_or_settles_writes_nothing() {
    for body in [
        "const o = { f: mk() }; void o;",
        "const xs = [mk()]; void xs;",
    ] {
        for effect in [false, true] {
            assert!(
                loops(body, effect).is_empty(),
                "{body} (effect: {effect}): {:?}",
                loops(body, effect)
            );
        }
    }
    let src = "import { useEffect, useState } from 'react';\n\
               export function App() {\n\
                 const [n, setN] = useState(0);\n\
                 const mk = () => () => setN(1);\n\
                 useEffect(() => { const o = { f: mk() }; o.f(); });\n\
                 return <div>{n}</div>;\n\
               }\n";
    assert!(findings_of(src, "settle").is_empty(), "{src}");
}

/// A ref's initial value is a render expression: the closure the call
/// returns is what `current` holds after the first render.
#[test]
fn a_returned_closure_is_a_refs_initial_value() {
    let src = "import { useRef, useState } from 'react';\n\
               export function App() {\n\
                 const [n, setN] = useState(0);\n\
                 const mk = () => () => setN(n + 1);\n\
                 const r = useRef(mk());\n\
                 r.current();\n\
                 return <div>{n}</div>;\n\
               }\n";
    assert!(!findings_of(src, "ref").is_empty(), "{src}");
}

/// A component that calls a prop it was handed as a call's value runs the
/// returned closure in its render.
#[test]
fn a_returned_closure_handed_as_a_prop_runs_in_the_child() {
    let src = "import { useState } from 'react';\n\
               export function Child({ onX }: { onX: () => void }) {\n\
                 onX();\n\
                 return <div />;\n\
               }\n\
               export function App() {\n\
                 const [n, setN] = useState(0);\n\
                 const mk = () => () => setN(n + 1);\n\
                 return <Child onX={mk()} />;\n\
               }\n";
    assert!(!findings_of(src, "prop").is_empty(), "{src}");
}
