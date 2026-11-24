//! A hand-over row is the row of one copy of a statement (#390): two
//! components that splice one hook statement hold two copies of it, and what
//! one copy hands over is never read as the other's. A component that
//! splices the hook twice keeps what both of its copies hand over.

use std::process::Command;

/// `(component, setter)` of every `cross-setter-in-render` row reactant
/// prints for `tests/fixtures/handover_rows/<dir>`, sorted.
fn cross_setters(dir: &str) -> Vec<(String, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg(format!("tests/fixtures/handover_rows/{dir}"))
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
        .expect("run reactant binary");
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let mut rows: Vec<(String, String)> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"] == "cross-setter-in-render")
        .flat_map(|d| {
            let var = d["var"].as_str().unwrap_or("").to_string();
            d["components"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |c| (c.as_str().unwrap().to_string(), var.clone()))
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

fn row(component: &str, setter: &str) -> (String, String) {
    (component.to_string(), setter.to_string())
}

#[test]
fn a_child_is_told_only_the_setters_its_own_copy_hands_over() {
    let rows = cross_setters("shared_hook");
    // `ChildB` never holds `ParentA`'s `setOpen`, nor `ChildA` `ParentB`'s
    // `setShown`: each was handed over by the other child's copy alone.
    assert!(!rows.contains(&row("ChildB", "setOpen")), "{rows:?}");
    assert!(!rows.contains(&row("ChildA", "setShown")), "{rows:?}");
    assert!(!rows.contains(&row("ChildA", "setLeft")), "{rows:?}");
    // What each child's own copy hands over stays.
    assert!(rows.contains(&row("ChildA", "setOpen")), "{rows:?}");
    assert!(rows.contains(&row("ChildB", "setShown")), "{rows:?}");
}

#[test]
fn a_hook_spliced_twice_keeps_what_both_copies_hand_over() {
    let rows = cross_setters("shared_hook");
    let twice: Vec<_> = rows.iter().filter(|(c, _)| c == "Twice").collect();
    assert_eq!(
        twice,
        [&row("Twice", "setLeft"), &row("Twice", "setRight")],
        "{rows:?}"
    );
}
