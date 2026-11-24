//! A hand-over row is the row of one copy of a statement (#390): two
//! components that splice one hook statement hold two copies of it, and what
//! one copy hands over is never read as the other's. A component that
//! splices the hook twice keeps what both of its copies hand over. One
//! hand-over site of a component is one row listing every setter it hands
//! (#377).

use std::process::Command;

/// The JSON rows reactant prints for `tests/fixtures/handover_rows/<dir>`.
fn rows(dir: &str) -> Vec<serde_json::Value> {
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
    doc["diagnostics"].as_array().unwrap().clone()
}

/// The names a row stands for: each of a hand-over row's `setters` (#377),
/// by the name the component holds it by, else the row's `var`.
fn vars(d: &serde_json::Value) -> Vec<String> {
    match d["setters"].as_array() {
        Some(setters) => setters
            .iter()
            .map(|s| s["var"].as_str().unwrap_or("").to_string())
            .collect(),
        None => vec![d["var"].as_str().unwrap_or("").to_string()],
    }
}

/// `(component, setter)` of every `cross-setter-in-render` row reactant
/// prints for `tests/fixtures/handover_rows/<dir>`, sorted.
fn cross_setters(dir: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = rows(dir)
        .iter()
        .filter(|d| d["rule"] == "cross-setter-in-render")
        .flat_map(|d| {
            let vars = vars(d);
            d["components"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(move |c| {
                    let c = c.as_str().unwrap().to_string();
                    vars.clone().into_iter().map(move |v| (c.clone(), v))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

/// The `setter-in-render` / `cross-setter-in-render` rows of `component`
/// in `grouped`: (rule, line, message, `setters` as (name, parent)).
type Row = (String, u64, String, Vec<(String, Option<String>)>);
fn grouped(component: &str) -> Vec<Row> {
    rows("grouped")
        .iter()
        .filter(|d| d["component"] == component)
        .filter(|d| d["rule"] == "setter-in-render" || d["rule"] == "cross-setter-in-render")
        .map(|d| {
            let setters = d["setters"].as_array().map_or_else(Vec::new, |s| {
                s.iter()
                    .map(|s| {
                        (
                            s["name"].as_str().unwrap().to_string(),
                            s["parent"].as_str().map(str::to_string),
                        )
                    })
                    .collect()
            });
            (
                d["rule"].as_str().unwrap().to_string(),
                d["line"].as_u64().unwrap(),
                d["message"].as_str().unwrap().to_string(),
                setters,
            )
        })
        .collect()
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

fn setter(name: &str, parent: Option<&str>) -> (String, Option<String>) {
    (name.to_string(), parent.map(str::to_string))
}

#[test]
fn one_hand_over_site_is_one_row_listing_every_setter_of_every_parent() {
    // #377: `register({ onA, onB })` hands two setters of each of two parents.
    let rows = grouped("Child");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (rule, line, message, setters) = &rows[0];
    assert_eq!((rule.as_str(), *line), ("cross-setter-in-render", 10));
    assert_eq!(
        setters,
        &[
            setter("setA", Some("ParentOne")),
            setter("setB", Some("ParentOne")),
            setter("setC", Some("ParentTwo")),
            setter("setD", Some("ParentTwo")),
        ]
    );
    assert!(
        message.starts_with(
            "4 state setters of parent components are handed to a callee with no timing \
             summary (`setA` of `ParentOne`, `setB` of `ParentOne`, `setC` of `ParentTwo` \
             and 1 more)."
        ),
        "{message}"
    );
}

#[test]
fn two_hand_over_sites_are_two_rows() {
    let rows = grouped("TwoSites");
    let at: Vec<_> = rows.iter().map(|(_, l, _, s)| (*l, s.clone())).collect();
    assert_eq!(
        at,
        [
            (40, vec![setter("setX", None)]),
            (41, vec![setter("setY", None)])
        ],
        "{rows:?}"
    );
    // One setter reads as before the grouping.
    assert!(
        rows[0].2.starts_with(
            "setter `setX` is handed to a callee with no timing summary. If that callee \
             runs it during render, this re-renders on every render"
        ),
        "{rows:?}"
    );
}

#[test]
fn a_setter_called_during_render_keeps_its_own_row() {
    // The call beside the hand-over is its own row, worded as a call.
    let rows = grouped("CallAndHandOver");
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows[0].3,
        [setter("setQ", None), setter("setR", None)],
        "{rows:?}"
    );
    let call = &rows[1].2;
    assert!(
        call.starts_with("setter `setP` called directly in the render body"),
        "{call}"
    );
    assert!(rows[1].3.is_empty(), "{rows:?}");
    // Two setters called at one site stay two rows: only a hand-over groups.
    let rows = grouped("CallsTwo");
    let called: Vec<_> = rows
        .iter()
        .map(|(_, l, m, s)| (*l, m.contains("called directly"), s.is_empty()))
        .collect();
    assert_eq!(called, [(60, true, true), (60, true, true)], "{rows:?}");
}
