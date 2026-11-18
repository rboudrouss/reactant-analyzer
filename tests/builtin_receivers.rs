//! A builtin the analysis does not model is handed its receiver as well as
//! its arguments (#330): `h.bind(null)`, `[h].slice()` may hand back what the
//! receiver holds, so it escapes there and may run. A builtin whose result is
//! the receiver (`sort`, `reverse`) or one of its entries (`at`, `find`,
//! `pop`) points where those do instead, and one whose result holds nothing
//! of it (`forEach`, `includes`) keeps it. Invariant I (design-walk §3):
//! every function value is placed or escaped.

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

/// The loop findings (`setter-in-render`, `infinite-loop`) of `App`, whose
/// render (`effect` false) or effect on `[n]` runs `body`.
fn loops(body: &str, effect: bool) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "reactant-recv-{}-{}-{}",
        std::process::id(),
        effect,
        body.len() as u64 * 31 + body.bytes().map(u64::from).sum::<u64>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let run = if effect {
        format!("useEffect(() => {{ {body} }}, [n]);")
    } else {
        body.to_string()
    };
    let src = format!(
        "import {{ useEffect, useState }} from 'react';\n\
         export function App() {{\n\
           const [n, setN] = useState(0);\n\
           const h = () => setN(n + 1);\n\
           const g = () => 0;\n\
           {run}\n\
           return <div>{{n}}{{typeof g}}</div>;\n\
         }}\n"
    );
    std::fs::write(dir.join("a.tsx"), src).unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"] == "setter-in-render" || d["rule"] == "infinite-loop")
        .map(|d| {
            format!(
                "{}:{}",
                d["rule"].as_str().unwrap(),
                d["severity"].as_str().unwrap()
            )
        })
        .collect()
}

/// React loops on each: the closure the builtin hands back, or runs, writes
/// the slot at every pass.
#[test]
fn a_closure_through_a_builtin_is_a_write_in_both_phases() {
    for body in [
        // An entry of the receiver.
        "const q = [h].at(0)!; q();",
        "const q = [h].find(() => true)!; q();",
        "const q = [h].pop()!; q();",
        "[h].at(0)!();",
        "const run = (f: any) => f(); run([h].at(0));",
        // The receiver itself, rearranged.
        "const q = [g, h].reverse()[0]; q();",
        "const q = [g, h].sort(() => 0)[1]; q();",
        // A builtin the value layer does not model: the receiver escapes.
        "const g2 = h.bind(null); g2();",
        "h.call(null);",
        "h.apply(null, []);",
        "const q = [h].slice()[0]; q();",
        "const q = [h].filter(Boolean)[0]; q();",
        "const q = [h].concat([])[0]; q();",
        "for (const f of [h].values()) f();",
        "const q = [h].reduce((a: any) => a); q();",
        // `for…in` keys read every member (#307's rule, on an object).
        "const fns: any = { a: g, b: h }; for (const k in fns) fns[k]();",
    ] {
        for effect in [false, true] {
            assert!(
                !loops(body, effect).is_empty(),
                "{body} (effect: {effect}): silent"
            );
        }
    }
}

/// The neighbours React settles: the closure is handed back and never
/// called, or the escaped function writes nothing.
#[test]
fn a_builtin_that_only_holds_the_closure_writes_nothing() {
    for body in [
        "const q = [h].at(0); void q;",
        "const q = [h].pop(); void q;",
        "const b = ((x: number) => x + 1).bind(null); void b(1);",
        "[h].forEach(() => {});",
        "void [h].includes(0);",
    ] {
        for effect in [false, true] {
            assert!(
                loops(body, effect).is_empty(),
                "{body} (effect: {effect}): {:?}",
                loops(body, effect)
            );
        }
    }
}
