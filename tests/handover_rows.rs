//! A hand-over row is the row of one copy of a statement (#390): two
//! components that splice one hook statement hold two copies of it, and what
//! one copy hands over is never read as the other's. A component that
//! splices the hook twice keeps what both of its copies hand over. One
//! hand-over site of a component is one row listing every setter it hands
//! (#377), at its level: an Info for package code, a Warning otherwise
//! (#394), and the closing line of a run says what was not shown.

use std::process::Command;

/// The output of reactant on `tests/fixtures/handover_rows/<dir>`, with
/// `extra` arguments.
fn run(dir: &str, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg(format!("tests/fixtures/handover_rows/{dir}"))
        .args([
            "--config",
            "tests/fixtures/all-rules.json",
            "--fail-on",
            "never",
        ])
        .args(extra)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json(dir: &str, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["--format", "json"];
    args.extend_from_slice(extra);
    serde_json::from_str(&run(dir, &args)).expect("JSON")
}

/// The JSON rows reactant prints for `tests/fixtures/handover_rows/<dir>`
/// under `--info`, where a hand-over to package code, an Info, is listed.
fn rows(dir: &str) -> Vec<serde_json::Value> {
    json(dir, &["--info"])["diagnostics"]
        .as_array()
        .unwrap()
        .clone()
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
/// in `grouped`: (rule, line, message, `setters` as (name, parent)). Its
/// callee, `register`, is a function the program declares, not a package's:
/// every row is a Warning, a hand-over included.
type Row = (String, u64, String, Vec<(String, Option<String>)>);
fn grouped(component: &str) -> Vec<Row> {
    rows("grouped")
        .iter()
        .filter(|d| d["component"] == component)
        .filter(|d| d["rule"] == "setter-in-render" || d["rule"] == "cross-setter-in-render")
        .inspect(|d| assert_eq!(d["severity"], "warning", "{d}"))
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
    let (handed, called): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| !r.3.is_empty());
    assert_eq!(
        handed[0].3,
        [setter("setQ", None), setter("setR", None)],
        "{rows:?}"
    );
    let call = &called[0].2;
    assert!(
        call.starts_with("setter `setP` called directly in the render body"),
        "{call}"
    );
    // Two setters called at one site stay two rows: only a hand-over groups.
    let rows = grouped("CallsTwo");
    let called: Vec<_> = rows
        .iter()
        .map(|(_, l, m, s)| (*l, m.contains("called directly"), s.is_empty()))
        .collect();
    assert_eq!(called, [(60, true, true), (60, true, true)], "{rows:?}");
}

/// `(component, line, severity)` of every hand-over row of `package`.
fn package_rows() -> Vec<(String, u64, String)> {
    let mut out: Vec<(String, u64, String)> = rows("package")
        .iter()
        .filter(|d| d["setters"].is_array())
        .map(|d| {
            (
                d["component"].as_str().unwrap().to_string(),
                d["line"].as_u64().unwrap(),
                d["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn handed(component: &str, line: u64, severity: &str) -> (String, u64, String) {
    (component.to_string(), line, severity.to_string())
}

/// A hand-over row is an Info only where the callee is provably a package's:
/// a function or a namespace member imported from a specifier the resolver
/// maps to no file, a component imported so (#389) or a member element of a
/// namespace imported so. A function the program declares, a builtin the
/// analysis does not model, a program context's provider, a name a body
/// rebinds: Warnings, as before the Info level.
#[test]
fn only_a_hand_over_to_package_code_is_an_info() {
    assert_eq!(
        package_rows(),
        [
            handed("BothAtOneSite", 42, "info"),
            handed("BothAtOneSite", 42, "warning"),
            handed("Shadowed", 65, "warning"),
            handed("ToPackageCall", 16, "info"),
            handed("ToPackageCall", 17, "info"),
            handed("ToPackageChild", 24, "info"),
            handed("ToPackageMember", 51, "info"),
            handed("ToProgram", 32, "warning"),
            handed("ToProgram", 33, "warning"),
            handed("ToProgramProvider", 58, "warning"),
        ]
    );
    // The Info says where the setter went; the Warning keeps S39's words.
    for d in rows("package").iter().filter(|d| d["setters"].is_array()) {
        let message = d["message"].as_str().unwrap();
        match d["severity"].as_str().unwrap() {
            "info" => assert!(message.contains("to package code the analysis does not see")),
            _ => assert!(message.contains("to a callee with no timing summary")),
        }
    }
}

/// One statement handing a setter to package code and another to the
/// program is two rows: a group never shows a Warning's setter at Info.
#[test]
fn a_site_handing_to_both_is_two_rows_one_per_level() {
    let at: Vec<(String, Vec<String>)> = rows("package")
        .iter()
        .filter(|d| d["component"] == "BothAtOneSite" && d["setters"].is_array())
        .map(|d| {
            let names = d["setters"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["name"].as_str().unwrap().to_string())
                .collect();
            (d["severity"].as_str().unwrap().to_string(), names)
        })
        .collect();
    assert_eq!(
        at,
        [
            ("warning".to_string(), vec!["setH".to_string()]),
            ("info".to_string(), vec!["setF".to_string()]),
        ]
    );
}

/// Without `--info` the Info rows are not shown and the output says how many
/// it hides: the human summary counts the setters handed to package code and
/// their sites, the JSON summary every hidden Info row, the hand-over rows
/// and their setters. The Error and Warning rows are the same either way.
#[test]
fn the_default_output_counts_the_hand_overs_it_does_not_show() {
    let all = json("package", &["--info"]);
    let shown = json("package", &[]);
    let at = |doc: &serde_json::Value, info: bool| -> Vec<serde_json::Value> {
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| (d["severity"] == "info") == info)
            .cloned()
            .collect()
    };
    assert_eq!(
        at(&shown, false),
        at(&all, false),
        "the Error and Warning rows"
    );
    assert!(at(&shown, true).is_empty(), "{shown}");
    let infos = at(&all, true);
    assert_eq!(shown["summary"]["hidden_infos"], infos.len(), "{shown}");
    assert_eq!(all["summary"]["infos"], infos.len(), "{all}");
    assert_eq!(all["summary"]["hidden_infos"], 0, "{all}");

    let hand_overs: Vec<_> = infos.iter().filter(|d| d["setters"].is_array()).collect();
    let setters: usize = hand_overs
        .iter()
        .map(|d| d["setters"].as_array().unwrap().len())
        .sum();
    // `ToPackageCall`'s two sites, `ToPackageChild`'s, `ToPackageMember`'s,
    // `BothAtOneSite`'s Info.
    assert_eq!((hand_overs.len(), setters), (5, 5), "{all}");
    assert_eq!(shown["summary"]["hidden_hand_overs"], 5, "{shown}");
    assert_eq!(shown["summary"]["hidden_hand_over_setters"], 5, "{shown}");
    let human = run("package", &[]);
    assert!(!human.contains("is handed to package code"), "{human}");
    let others = match infos.len() - hand_overs.len() {
        0 => String::new(),
        n => format!(" and {n} other info finding(s)"),
    };
    let line = format!(
        "5 setter(s) handed to package code the analysis does not see (5 site(s)){others} are \
         not shown, rerun with --info to list them"
    );
    assert!(human.contains(&line), "{line}\n{human}");
    assert!(!run("package", &["--info"]).contains("not shown, rerun with --info"));
}

/// The levels of the hand-over rows of `members/<dir>`, one program each:
/// the fact that decides them is the program's.
fn member_levels(dir: &str) -> Vec<String> {
    rows(&format!("members/{dir}"))
        .iter()
        .filter(|d| d["setters"].is_array())
        .map(|d| d["severity"].as_str().unwrap().to_string())
        .collect()
}

/// What an import of a package reads is that package's code, the premise the
/// registry stands on, unless the program writes into a package import: a
/// write whose receiver is rooted at one (`ext.hook = …`, a namespace's
/// member, another package's `other.flag = 1`) makes every package import
/// read as the program's. Module interop reads a bare name and a namespace
/// member off the package's mutable exports object too. A write into
/// anything else (`window.onfoo = …`, a literal, a parameter that shadows
/// the import) does not count, nor does a primitive literal written into
/// one, which installs no code (#403); `Object.assign` counts under any
/// spelling of its receiver. A write through an alias of the import is the
/// documented premise: not seen.
#[test]
fn a_package_import_is_the_packages_unless_the_program_writes_into_one() {
    for (dir, levels) in [
        ("bare", vec!["info"]),
        ("chain_intact", vec!["info", "info"]),
        ("chain_replaced", vec!["warning"]),
        ("namespace_replaced", vec!["warning"]),
        ("bare_replaced", vec!["warning"]),
        ("chain_unrelated_write", vec!["info"]),
        ("other_package_write", vec!["warning"]),
        ("alias_write", vec!["info"]),
        ("shadowed", vec!["warning"]),
        ("call_result", vec!["warning"]),
        ("primitive_write", vec!["info"]),
        ("param_shadow_write", vec!["info"]),
        ("assign_destructured", vec!["warning"]),
        ("array_isarray", vec![]),
        ("array_shadowed", vec!["warning"]),
    ] {
        assert_eq!(member_levels(dir), levels, "{dir}");
    }
}

/// The closing line of a run (the user's rule, 2026-11-24): the tick only
/// when nothing was found and nothing hidden; a run that shows nothing but
/// hid Info rows says it is not exhaustive and how to list what it met; a
/// run that shows findings says what it hid under them.
#[test]
fn the_closing_line_says_when_the_analysis_was_not_exhaustive() {
    // Nothing shown, nothing hidden.
    let clean = run("summary_clean", &[]);
    assert!(clean.contains("✓  1 file(s) no issues found."), "{clean}");
    assert!(!clean.contains("not shown"), "{clean}");

    // Nothing shown, an Info hidden.
    let hidden = run("summary_hidden", &[]);
    assert!(!hidden.contains("no issues found"), "{hidden}");
    assert!(
        hidden.contains(
            "⚠  1 file(s), no errors or warnings, but the analysis is not exhaustive: it met 1 \
             limit(s), 1 setter(s) handed to package code the analysis does not see (1 site(s)); \
             rerun with --info to list them."
        ),
        "{hidden}"
    );
    // Listed under `--info`, it is still a limit the run met.
    let listed = run("summary_hidden", &["--info"]);
    assert!(!listed.contains("no issues found"), "{listed}");
    assert!(
        listed.contains("no errors or warnings, but the analysis is not exhaustive: it met 1 limit(s), listed above."),
        "{listed}"
    );

    // A Warning shown, an Info hidden.
    let both = run("summary_both", &[]);
    assert!(both.contains("⚠  1 warning(s) across 1 file(s)."), "{both}");
    assert!(
        both.contains(
            "1 setter(s) handed to package code the analysis does not see (1 site(s)) is not \
             shown, rerun with --info to list them"
        ),
        "{both}"
    );
}

/// A component whose only rows are hidden Infos met a limit of the analysis:
/// the count of hidden components never calls it clean.
#[test]
fn a_component_that_met_a_limit_is_not_counted_clean() {
    let clean = run("summary_clean", &[]);
    assert!(
        clean.contains("1 clean component(s) hidden, rerun with --show-clean"),
        "{clean}"
    );
    let hidden = run("summary_hidden", &[]);
    assert!(!hidden.contains("clean component"), "{hidden}");
    assert!(
        hidden.contains(
            "1 component(s) with no error or warning hidden, 1 of which met a limit of the \
             analysis; rerun with --show-clean"
        ),
        "{hidden}"
    );
    let shown = run("summary_hidden", &["--show-clean"]);
    assert!(
        shown.contains("no errors or warnings, 1 info finding(s) not shown (--info)"),
        "{shown}"
    );
}
