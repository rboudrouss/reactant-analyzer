//! A component mounted through an element the tree cannot place is not a
//! root that stays mounted (#313): the element may sit under a guard its own
//! loop moves, so a loop through it is never certain.

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

/// One directory per call: two tests of this binary run at the same time,
/// and a directory keyed by the program's text alone is removed under the
/// other test's analyzer (an empty stdout, "EOF while parsing a value").
static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The `infinite-loop` severities of the program `src`.
fn loops(src: &str) -> Vec<String> {
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reactant-unplaced-{}-{run}", std::process::id()));
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
        .filter(|d| d["rule"] == "infinite-loop")
        .map(|d| d["severity"].as_str().unwrap().to_string())
        .collect()
}

/// `C` stores a new object after every render and ticks its parent, which
/// renders it as `element` while `n < 3`.
fn mounted(decls: &str, element: &str) -> Vec<String> {
    loops(&format!(
        "import {{ useEffect, useState }} from 'react';\n\
         function C({{ onTick }}) {{\n\
           const [s, setS] = useState({{}});\n\
           useEffect(() => {{ setS({{}}); onTick(); }});\n\
           return <div>{{Object.keys(s).length}}</div>;\n\
         }}\n\
         function D() {{ return <span />; }}\n\
         {decls}\n\
         export function App() {{\n\
           const [n, setN] = useState(0);\n\
           const tick = () => setN((x) => x + 1);\n\
           const Comp = n >= 0 ? C : D;\n\
           const items = n < 3 ? [C] : [];\n\
           return <div>{{n < 3 ? {element} : null}}{{typeof Comp}}{{items.length}}</div>;\n\
         }}\n"
    ))
}

#[test]
fn a_child_an_unplaced_element_mounts_is_no_certain_loop() {
    // React settles on each: the parent stops rendering the child.
    for (decls, element) in [
        ("", "<Comp onTick={tick} />"),
        ("const parts = { C };", "<parts.C onTick={tick} />"),
        ("", "items.map((X, i) => <X key={i} onTick={tick} />)"),
    ] {
        let found = mounted(decls, element);
        assert!(!found.is_empty(), "{element}: a possible loop");
        assert!(found.iter().all(|s| s != "error"), "{element}: {found:?}");
    }
}
