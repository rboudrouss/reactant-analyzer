//! `setter-in-render` reads the writer relation, which reads the call
//! relation (ADR-047): what a render call writes is what the fixpoint ran
//! there, never a name table. An Error takes the whole proof: the row is
//! placed — every evaluation of the call surely wrote that one slot, in the
//! render, at least once per pass of its block — and its block dominates
//! every exit. A guarded write settles only on a guard read where the write
//! runs (#269), never on a name the program assigns elsewhere (#314.2).

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

/// The `setter-in-render` severities of `App`, whose render runs `body`.
fn levels(body: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "reactant-sir-{}-{}",
        std::process::id(),
        body.len() as u64 * 31 + body.bytes().map(u64::from).sum::<u64>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = format!(
        "import {{ useState }} from 'react';\n\
         import {{ someCall, noop }} from 'ext';\n\
         export function App(props: {{ c: boolean }}) {{\n\
           const [n, setN] = useState(0);\n\
           const [m, setM] = useState(0);\n\
           {body}\n\
           return <div>{{n}}{{m}}</div>;\n\
         }}\n"
    );
    std::fs::write(dir.join("a.tsx"), src).unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let mut levels: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"] == "setter-in-render")
        .map(|d| d["severity"].as_str().unwrap().to_string())
        .collect();
    levels.sort();
    levels
}

#[test]
fn an_unconditional_render_write_is_an_error() {
    assert_eq!(levels("setN(1);"), ["error"]);
    // An alias is the same write (#176).
    assert_eq!(levels("const s = setN; s(1);"), ["error"]);
    // Through a helper with one calling context, a rest parameter, an array
    // method's callback the receiver surely runs.
    assert_eq!(
        levels("const run = (f: any) => f(n + 1); run(setN);"),
        ["error"]
    );
    assert_eq!(
        levels("const run = (...fs: any[]) => fs[0](n + 1); run(setN);"),
        ["error"]
    );
    assert_eq!(
        levels("const cb = (set: any) => set(n + 1); [setN].forEach(cb);"),
        ["error"]
    );
}

#[test]
fn a_write_not_proven_to_run_or_to_write_is_a_warning() {
    assert_eq!(levels("if (props.c) setN(42);"), ["warning"]);
    // Handed to a callee the analysis does not see: the hand-over row, an
    // Info.
    assert_eq!(levels("someCall((u: number) => setN(u));"), ["info"]);
    // A callee that may be the setter or something else.
    assert_eq!(
        levels("const f = props.c ? setN : noop; f(1);"),
        ["warning"]
    );
}

#[test]
fn a_name_rebound_before_the_call_is_what_it_holds_there() {
    // `s` is the no-op at the call: nothing is written.
    assert!(levels("let s: any = setN; s = noop; s(1);").is_empty());
}

#[test]
fn each_written_slot_is_a_finding() {
    assert_eq!(levels("setN(1); setM(2);"), ["error", "error"]);
    assert_eq!(
        levels("[setN, setM].forEach((set) => set(n + m + 1));"),
        ["warning", "warning"]
    );
}

#[test]
fn a_guard_settles_only_on_the_values_its_write_reads() {
    // #269: the callback's guard dies once `1` sits in the slot.
    assert!(levels("[1].forEach(() => { if (n !== 1) setN(1); });").is_empty());
    // A guard on the callback's own parameter is no render name: no proof.
    assert_eq!(
        levels("[1].forEach((k) => { if (n !== k) setN(n + k); });"),
        ["warning"]
    );
}

/// #331: `n !== x` dies once `x` sits in the slot only when the next pass
/// computes the same `x`. A value read off the slot, a name some function
/// assigns, a memo of the slot, a call that may return another value, or a
/// slot another render write moves is recomputed between the passes: React
/// loops, the write stays a finding.
#[test]
fn a_write_settles_only_on_a_value_that_holds_between_passes() {
    for body in [
        "const x = n + 1; if (n !== x) setN(x);",
        "let x = 0; x = n + 1; if (n !== x) setN(x); x = 0;",
        "const x = n + 1; [1].forEach(() => { if (n !== x) setN(x); });",
        "let x = 0; const f = () => { x = n + 1; }; f(); if (n !== x) setN(x);",
        "const x = Math.random(); if (n !== x) setN(x);",
        "if (m !== n + 1) setM(n + 1); if (n !== m) setN(m);",
    ] {
        assert!(!levels(body).is_empty(), "{body}: silent");
    }
    // The value the guard compares holds: a literal, a prop, a pure builtin
    // of a prop, a slot no render write moves.
    for body in [
        "if (n !== 0) setN(0);",
        "const x = Number(props.c); if (n !== x) setN(x);",
        "const x = Math.max(Number(props.c), 1); if (n !== x) setN(x);",
        "if (n !== m) setN(m);",
    ] {
        assert!(levels(body).is_empty(), "{body}: {:?}", levels(body));
    }
}

/// A callback's defaulted parameter is its own: `({ k } = {}) => …` stores
/// into the callback's `__p0`, not into a destructured component parameter
/// lowered under the same name, which still holds across the passes.
#[test]
fn a_callback_default_parameter_moves_no_prop() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reactant-sir-p0-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.tsx"),
        "import { useCallback, useState } from 'react';\n\
         export function Other() {\n\
           const f = useCallback(({ k }: any = {}) => k, []);\n\
           return <div onClick={() => f()} />;\n\
         }\n\
         export function App({ k }: { k: number }) {\n\
           const [m, setM] = useState('');\n\
           const x = String(k);\n\
           if (m !== x) setM(x);\n\
           return <div>{m}</div>;\n\
         }\n",
    )
    .unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let rows: Vec<&serde_json::Value> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"] == "setter-in-render")
        .collect();
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn a_memo_of_the_slot_moves_between_passes() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reactant-sir-memo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.tsx"),
        "import { useMemo, useState } from 'react';\n\
         export function App() {\n\
           const [n, setN] = useState(0);\n\
           const x = useMemo(() => n + 1, [n]);\n\
           if (n !== x) setN(x);\n\
           return <div>{n}</div>;\n\
         }\n",
    )
    .unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    assert!(
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["rule"] == "setter-in-render"),
        "{doc}"
    );
}
