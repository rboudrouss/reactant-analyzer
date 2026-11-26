//! One loop row per anchor site (#421): the Warning rows of one loop rule and
//! message family at one effect are one row, within a component and across
//! the components that inline the effect, standing for every cycle of them
//! in a `cycles` field; each cycle's steps say what the analysis could not
//! prove and why. An effect calling parents' setters is one row listing them.

use std::process::Command;

fn json(dir: &str, extra: &[&str]) -> Vec<serde_json::Value> {
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg(format!("tests/fixtures/loop_rows/{dir}"))
        .args([
            "--config",
            "tests/fixtures/all-rules.json",
            "--fail-on",
            "never",
            "--format",
            "json",
        ])
        .args(extra)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"].as_str().unwrap().contains("infinite-loop"))
        .cloned()
        .collect()
}

/// `(component, path)` of every cycle of `row`.
fn cycles(row: &serde_json::Value) -> Vec<(String, String)> {
    row["cycles"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(|c| {
                    (
                        c["component"].as_str().unwrap().to_string(),
                        c["path"].as_str().unwrap().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn one_effect_site_closing_two_cycles_is_one_row_listing_both() {
    // Four copies of one effect at one site of `App`: two cycles, one row,
    // the same under `--per-component` (the rule groups a component's rows).
    for extra in [&[][..], &["--per-component"]] {
        let rows = json("one_site", extra);
        assert_eq!(rows.len(), 1, "{rows:#?}");
        let row = &rows[0];
        assert_eq!(row["severity"], "warning");
        assert_eq!(
            cycles(row),
            vec![
                ("App".to_string(), "`x` → `y` → `x`".to_string()),
                ("App".to_string(), "`z` → `w` → `z`".to_string()),
            ]
        );
        let msg = row["message"].as_str().unwrap();
        assert!(
            msg.starts_with("these effects may form a state-update cycle (`x` → `y` → `x`)")
                && msg.contains(". Not proven: ")
                && msg.ends_with("1 more cycle goes through this effect: (`z` → `w` → `z`)"),
            "{msg}"
        );
        // Each step: who writes what, where, how it re-runs, and why the
        // analysis could not prove it, from the engine's own reason.
        let step = &row["cycles"][0]["steps"][0];
        assert_eq!(step["component"], "App");
        assert_eq!(step["from"]["state"], "x");
        assert_eq!(step["state"]["state"], "y");
        assert_eq!(step["state"]["component"], "App");
        assert_eq!(step["reruns"], "dep");
        assert_eq!(step["unproven"]["kind"], "value");
        assert!(step["line"].as_u64().is_some());
    }
}

#[test]
fn a_site_two_components_inline_is_one_row_naming_each_cycles_component() {
    // `useSync` closes a cycle in `A` and another in `B`: one row at its
    // effect, through both components, listing both cycles, each with its
    // component. Per component, each keeps its own row and its own cycle:
    // the claims the oracle judges do not move.
    let rows = json("shared", &[]);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["components"], serde_json::json!(["A", "B"]));
    assert_eq!(
        cycles(&rows[0]),
        vec![
            ("A".to_string(), "`x` → `y` → `x`".to_string()),
            ("B".to_string(), "`p` → `q` → `p`".to_string()),
        ]
    );
    assert!(
        rows[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("1 more cycle goes through this effect: (`p` → `q` → `p`) in `B`")
    );
    let per = json("shared", &["--per-component"]);
    let per: Vec<(String, Vec<(String, String)>)> = per
        .iter()
        .map(|r| (r["component"].as_str().unwrap().to_string(), cycles(r)))
        .collect();
    assert_eq!(
        per,
        vec![
            (
                "A".to_string(),
                vec![("A".to_string(), "`x` → `y` → `x`".to_string())]
            ),
            (
                "B".to_string(),
                vec![("B".to_string(), "`p` → `q` → `p`".to_string())]
            ),
        ]
    );
}

#[test]
fn an_effect_calling_two_parent_setters_is_one_row_listing_them() {
    let rows = json("cross", &[]);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!(row["rule"], "cross-component-infinite-loop");
    let setters: Vec<(&str, &str)> = row["setters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["parent"].as_str().unwrap()))
        .collect();
    assert_eq!(setters, vec![("onA", "App"), ("onB", "App")]);
    let msg = row["message"].as_str().unwrap();
    assert!(
        msg.starts_with("this effect calls 2 state setters of parent components (`onA` of `App`, `onB` of `App`)")
            && msg.contains("if a re-render of `App` changes one of them"),
        "{msg}"
    );
    assert!(row.get("cycles").is_none());
}
