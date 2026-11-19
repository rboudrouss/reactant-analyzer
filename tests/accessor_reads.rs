//! A member read through a getter is a call of its body, and a write
//! through a setter accessor a call of the setter (#321): its return is what
//! the read is (a new object when every return allocates), its writes are
//! writes, and the body runs wherever the object's member is read, so it
//! escapes at the literal.

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

/// The loop findings (`rule:severity`) of the program `src`.
fn loops(src: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "reactant-accessor-{}-{}",
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
        .filter(|d| {
            let rule = d["rule"].as_str().unwrap_or("");
            rule.ends_with("setter-in-render") || rule == "infinite-loop"
        })
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

/// The `setter-in-render` messages reactant prints for the oracle case
/// `case`.
fn setter_messages(case: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "reactant-accessor-case-{}-{case}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("oracle/cases/{case}.tsx")),
    )
    .unwrap();
    std::fs::write(dir.join("a.tsx"), src).unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"].as_str() == Some("setter-in-render"))
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_state_write_in_an_accessor_says_the_read_or_the_write_runs_it() {
    for (case, says) in [
        (
            "getter-writes-state-in-render-loops",
            "setter `setN` is called in the getter of `p`, which runs at every read of `p`. \
             If the render reads it, this re-renders on every render",
        ),
        (
            "setter-accessor-writes-state-in-render-loops",
            "setter `setN` is called in the setter of `p`, which runs at every write of `p`. \
             If the render writes it, this re-renders on every render",
        ),
        (
            "setter-accessor-in-effect-loops",
            "setter `setN` is called in the setter of `p`, which runs at every write of `p`. \
             If the render writes it, this re-renders on every render",
        ),
    ] {
        assert_eq!(setter_messages(case), [says], "{case}");
    }
}

/// `App` with `decl` in its render and an effect on `[dep]` that bumps `n`.
fn effect_on(decl: &str, dep: &str) -> Vec<String> {
    loops(&format!(
        "import {{ useEffect, useState }} from 'react';\n\
         export function App() {{\n\
           const [n, setN] = useState(0);\n\
           {decl}\n\
           useEffect(() => {{ setN((c) => c + 1); }}, [{dep}]);\n\
           return <div>{{n}}</div>;\n\
         }}\n"
    ))
}

const ERROR: &str = "infinite-loop:error";
const WARNING: &str = "infinite-loop:warning";

#[test]
fn a_dep_read_through_an_allocating_getter_is_a_new_reference_every_render() {
    let o = "const o = { get p() { return {}; } };";
    assert_eq!(effect_on(o, "o.p"), [ERROR], "the read itself");
    assert_eq!(
        effect_on(&format!("{o} const d = o.p;"), "d"),
        [ERROR],
        "bound to a name first"
    );
    assert_eq!(
        effect_on(&format!("{o} const {{ p }} = o;"), "p"),
        [ERROR],
        "destructured"
    );
}

#[test]
fn the_plain_member_twin_is_unchanged() {
    assert_eq!(effect_on("const o = { p: {} };", "o.p"), [ERROR]);
}

#[test]
fn a_getter_returning_one_value_holds_still() {
    assert!(effect_on("const o = { get p() { return 1; } };", "o.p").is_empty());
}

#[test]
fn a_getter_the_analysis_cannot_bound_may_re_run_the_effect() {
    // `this` is not bound, and a computed name may be any member.
    assert_eq!(
        effect_on("const o = { v: {}, get p() { return this.v; } };", "o.p"),
        [WARNING]
    );
    assert_eq!(
        effect_on(
            "const k = 'p'; const o: any = { get [k]() { return {}; } };",
            "o.p"
        ),
        [WARNING]
    );
}

#[test]
fn a_class_getter_read_off_an_instance_the_render_builds_is_reported() {
    let found = loops(
        "import { useEffect, useState } from 'react';\n\
         class K { get p() { return {}; } }\n\
         export function App() {\n\
           const [n, setN] = useState(0);\n\
           const k = new K();\n\
           useEffect(() => { setN((c) => c + 1); }, [k.p]);\n\
           return <div>{n}</div>;\n\
         }\n",
    );
    assert!(!found.is_empty(), "{found:?}");
}

/// `src` with an `App` whose effect on `[dep]` bumps `n`, `decl` in its
/// render, after the module-level `module`.
fn module_effect_on(module: &str, decl: &str, dep: &str) -> Vec<String> {
    loops(&format!(
        "import {{ useEffect, useMemo, useState }} from 'react';\n\
         {module}\n\
         export function App() {{\n\
           const [n, setN] = useState(0);\n\
           {decl}\n\
           useEffect(() => {{ setN((c) => c + 1); }}, [{dep}]);\n\
           return <div>{{n}}</div>;\n\
         }}\n"
    ))
}

#[test]
fn a_getter_read_off_an_object_the_heap_does_not_place_may_re_run_the_effect() {
    // #334: a module literal, a class instance a memo keeps, a static getter.
    let literal = "const o = { get p() { return {}; } };";
    assert_eq!(module_effect_on(literal, "", "o.p"), [WARNING]);
    assert_eq!(
        module_effect_on(
            "class K { get p() { return {}; } }",
            "const k = useMemo(() => new K(), []);",
            "k.p"
        ),
        [WARNING]
    );
    assert_eq!(
        module_effect_on("class K { static get p() { return {}; } }", "", "K.p"),
        [WARNING]
    );
    // A getter returning one number settles; the analysis does not run a
    // getter it does not place, and warns.
    assert_eq!(
        module_effect_on("const o = { get p() { return 1; } };", "", "o.p"),
        [WARNING]
    );
}

#[test]
fn a_module_literal_handed_down_runs_its_getter_in_the_child() {
    let found = loops(
        "import { useEffect, useState } from 'react';\n\
         function C({ o }) {\n\
           const [n, setN] = useState(0);\n\
           useEffect(() => { setN((c) => c + 1); }, [o.p]);\n\
           return <div>{n}</div>;\n\
         }\n\
         const shared = { get p() { return {}; } };\n\
         export function App() { return <C o={shared} />; }\n",
    );
    assert_eq!(found, [WARNING]);
}

#[test]
fn a_getter_name_read_where_the_heap_places_a_plain_member_stays_precise() {
    let getter = "const g = { get p() { return {}; } }; export const G = g;";
    assert!(module_effect_on(getter, "const o = useMemo(() => ({ p: 1 }), []);", "o.p").is_empty());
    assert_eq!(
        module_effect_on(getter, "const o = { p: {} };", "o.p"),
        [ERROR]
    );
    // A name no accessor defines is computed from the object, as before.
    assert!(module_effect_on(getter, "", "g.q").is_empty());
}

#[test]
fn an_accessor_writing_state_runs_where_its_member_is_read_or_written() {
    let render = |decl: &str, use_: &str| {
        loops(&format!(
            "import {{ useState }} from 'react';\n\
             export function App() {{\n\
               const [n, setN] = useState(0);\n\
               const o = {decl};\n\
               {use_}\n\
               return <div>{{n}}</div>;\n\
             }}\n"
        ))
    };
    let setter = render("{ set p(v: number) { setN(v + 1); } }", "o.p = n;");
    assert!(
        setter.iter().any(|f| f.starts_with("setter-in-render")),
        "{setter:?}"
    );
    let getter = render(
        "{ get p() { setN(n + 1); return 0; } }",
        "const x = o.p; console.log(x);",
    );
    assert!(
        getter.iter().any(|f| f.starts_with("setter-in-render")),
        "{getter:?}"
    );
}
