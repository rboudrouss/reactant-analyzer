//! #130 and the two precision defects the wider walk exposed.
//!
//! The walk reaches a `Call` in every expression position, not only in
//! statement position — so a write nested in another call's argument, a JSX
//! prop or a ternary arm is in the writer relation. Three things had to hold
//! for that to be reportable:
//!
//! 1. the write is found at all (`setter_phase/App.tsx`);
//! 2. `setter-in-render` reads the phase the walk assigned it, so a proven
//!    deferred write is silent and a ⊤ one does not claim to be a direct call;
//! 3. a variable that is a setter only by *capture* cannot be certified
//!    (`setter_capture/App.tsx`), and a component's own setter is not a
//!    parent's just because its name was salted (`setter_identity/`).

use std::process::{Command, Output};

fn reactant(fixture: &str, extra: &[&str]) -> Output {
    let mut args = vec!["check", fixture, "--all-roots", "--fail-on", "never"];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(&args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn report(fixture: &str) -> String {
    String::from_utf8_lossy(&reactant(fixture, &[]).stdout).into_owned()
}

const PHASE: &str = "tests/fixtures/setter_phase";
const IDENTITY: &str = "tests/fixtures/setter_identity";
const CAPTURE: &str = "tests/fixtures/setter_capture";

/// One line of the report for the named component, or `""`.
fn line_for(out: &str, component: &str) -> String {
    let mut lines = out
        .lines()
        .skip_while(|l| !l.trim_start().starts_with(component));
    lines.next();
    lines.next().unwrap_or("").to_string()
}

#[test]
fn a_setter_call_in_another_calls_argument_is_a_render_write() {
    let out = report(PHASE);
    let line = line_for(&out, "NestedArgument");
    assert!(
        line.contains("error") && line.contains("setter-in-render"),
        "`wrap(setN(1))` is a write in the render pass, and an unconditional \
         one — the walk must reach a call that is not in statement position:\n{out}"
    );
}

#[test]
fn a_setter_call_inside_a_jsx_prop_value_is_a_render_write() {
    let out = report(PHASE);
    let line = line_for(&out, "NestedInJsxProp");
    assert!(
        line.contains("error") && line.contains("setter-in-render"),
        "a prop's value is evaluated during render:\n{out}"
    );
}

#[test]
fn a_callee_with_no_timing_summary_warns_without_claiming_a_direct_call() {
    let out = report(PHASE);
    let line = line_for(&out, "UnknownTiming");
    assert!(
        line.contains("warn") && line.contains("no timing summary"),
        "⊤ includes the render pass, so the row stays — but `called directly \
         in the render body` states the one thing the walk could not \
         establish:\n{out}"
    );
    assert!(
        !line.contains("called directly"),
        "the certain wording belongs to a `Sync` row only:\n{out}"
    );
}

#[test]
fn a_write_a_known_registrar_deferred_is_not_a_render_write() {
    let out = report(PHASE);
    assert!(
        !out.contains("DeferredWrite"),
        "`setTimeout(() => setN(1))` is proof the write is not in the render \
         pass — this rule has nothing to say about it:\n{out}"
    );
}

#[test]
fn a_components_own_setter_is_not_a_parents_when_its_name_was_salted() {
    let out = report(IDENTITY);
    assert!(
        !out.contains("cross-setter-in-render"),
        "two files define `Demo`, so both are keyed `Demo@<file>` — the owner \
         recorded on the setter must use that same spelling, or a component \
         reads as its own parent:\n{out}"
    );
}

#[test]
fn a_setter_a_closure_only_carries_cannot_be_certified() {
    let out = report(CAPTURE);
    let line = line_for(&out, "List");
    assert!(
        line.contains("cross-setter-in-render"),
        "the capture is still a may — the row stays:\n{out}"
    );
    assert!(
        line.contains("warn"),
        "`renderRows` puts `onPick` inside a JSX handler and never calls it: \
         dominance cannot upgrade a may to a must:\n{out}"
    );
}

#[test]
fn a_settling_render_write_is_not_verified_as_no_setter_call() {
    let stdout = reactant(PHASE, &["--info"]).stdout;
    let out = String::from_utf8_lossy(&stdout);
    let section: Vec<&str> = out
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("SettlingGuard"))
        .skip(1)
        .take_while(|l| l.starts_with("    "))
        .collect();
    assert!(
        !section
            .iter()
            .any(|l| l.contains("setter-in-render")
                && l.contains("no setter is called during render")),
        "`if (s) setS(null)` calls a setter during render; the rule drops it \
         because it settles, and may claim only that:\n{out}"
    );
    assert!(
        section.iter().any(|l| l.contains("verified")
            && l.contains("setter-in-render")
            && l.contains("fails to settle")),
        "the rule did check the component, and says what it proved:\n{out}"
    );
}

const NAMES: &str = "tests/fixtures/setter_names";

/// The `setter `…`` a component's `setter-in-render` row names.
fn named_setter(component: &str) -> String {
    let out = report(NAMES);
    let line = line_for(&out, component);
    let after = line
        .split_once("setter `")
        .unwrap_or_else(|| panic!("no setter named for {component}:\n{out}"))
        .1;
    after.split('`').next().unwrap_or("").to_string()
}

#[test]
fn a_setter_read_off_an_index_is_named_as_the_source_indexes_it() {
    assert_eq!(named_setter("IndexAccess"), "t[1]");
}

#[test]
fn a_setter_read_off_a_hook_value_keeps_its_member_path() {
    assert_eq!(named_setter("MemberOfHook"), "c.set");
}

#[test]
fn a_setter_returned_by_a_call_is_named_by_that_call() {
    assert_eq!(named_setter("CallOfHook"), "useCounter(…).set");
    assert_eq!(named_setter("CallOfUtility"), "pick(…)");
    assert_eq!(named_setter("CallOfLocalFn"), "getSetter(…)");
}

#[test]
fn the_trace_names_the_setter_as_the_message_does_and_json_keeps_the_variable() {
    let trace = String::from_utf8_lossy(&reactant(NAMES, &["--trace"]).stdout).into_owned();
    assert!(
        trace.contains("`getSetter(…)` is a state setter"),
        "the witness step names the callee by its call:\n{trace}"
    );
    let json = String::from_utf8_lossy(&reactant(NAMES, &["--format", "json"]).stdout).into_owned();
    assert!(
        json.contains("\"callee\":\"__call_") || json.contains("\"callee\": \"__call_"),
        "the machine-readable `callee` is the variable, unchanged:\n{json}"
    );
}
