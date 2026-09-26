//! End-to-end tests for `state-lifted-too-high`: a state used only inside one
//! child subtree of its owner, so every component above that subtree
//! re-renders on each write only to pass the value down.
//!
//! The `drill`, `hook_state` and `owner_*` fixtures come from
//! `scripts/rerender-bench`, where the re-render counts they encode were
//! measured at runtime (React 19, jsdom).

use std::process::Command;

const RULE: &str = "state-lifted-too-high";

fn findings(fixture: &str, extra: &[&str]) -> Vec<serde_json::Value> {
    let path = format!("tests/fixtures/state_lifted_too_high/{fixture}");
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(["check", &path, "--rule", RULE, "--format", "json"])
        .args(extra)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{fixture}: bad JSON ({e}); stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    doc["diagnostics"].as_array().expect("diagnostics").clone()
}

fn message(d: &serde_json::Value) -> &str {
    d["message"].as_str().expect("message")
}

#[test]
fn prop_drilled_three_levels_names_the_leaf() {
    let ds = findings("drill.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    let d = &ds[0];
    assert_eq!(d["severity"], "warning");
    assert_eq!(d["component"], "App");
    let m = message(d);
    assert!(m.contains("`<Field>`, 3 levels below `App`"), "{m}");
    assert!(
        m.contains("`App`, `Layout`, `Sidebar` and 1 other component they render"),
        "{m}"
    );
    let hops: Vec<_> = d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "forward")
        .collect();
    assert_eq!(hops.len(), 3, "one note per hop: {hops:#?}");
}

#[test]
fn the_fixed_version_is_silent() {
    assert!(findings("drill_fixed.tsx", &[]).is_empty());
}

#[test]
fn state_inside_an_inlined_custom_hook_is_seen() {
    let ds = findings("hook_state.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(message(&ds[0]).contains("state `search` is only used inside `<FilterSelect>`"));
}

/// The wrapper the real fix (dub#3633) introduced re-renders only itself: one
/// wasted render is under the default threshold, and reported when asked.
#[test]
fn a_one_render_wrapper_is_under_the_default_threshold() {
    assert!(findings("hook_wrapper.tsx", &[]).is_empty());
    let ds = findings(
        "hook_wrapper.tsx",
        &["--rule-option", "state-lifted-too-high:minWastedRenders=1"],
    );
    assert_eq!(ds.len(), 1, "{ds:#?}");
}

#[test]
fn min_depth_filters_shallow_homes() {
    let ds = findings(
        "drill.tsx",
        &["--rule-option", "state-lifted-too-high:minDepth=3"],
    );
    assert_eq!(ds.len(), 1);
    let ds = findings(
        "drill.tsx",
        &["--rule-option", "state-lifted-too-high:minDepth=4"],
    );
    assert!(ds.is_empty());
}

#[test]
fn the_home_stops_at_the_first_component_that_uses_it() {
    let ds = findings("intermediate_uses.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(
        message(&ds[0]).contains("`<Layout>`, 1 level below `App`"),
        "{}",
        message(&ds[0])
    );
}

#[test]
fn spreads_and_derived_values_are_followed() {
    let ds = findings("spread.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(message(&ds[0]).contains("`<Field>`, 2 levels below `App`"));
    let ds = findings("derived.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(message(&ds[0]).contains("`<Search>`, 1 level below `Page`"));
}

/// Each of these owners uses the state itself: through a controlled input,
/// a mount condition, an effect, a handler on its own host element, a host
/// element it hands to a child as `children`, or the element type it
/// renders (a component returned by a hook, closed over the state).
#[test]
fn owners_that_use_the_state_are_silent() {
    for f in [
        "owner_input.tsx",
        "owner_guard.tsx",
        "owner_effect.tsx",
        "handler_reads.tsx",
        "host_in_children.tsx",
        "component_from_hook.tsx",
    ] {
        assert!(findings(f, &[]).is_empty(), "{f}");
    }
}

/// Uses in two branches, and a child the analysis cannot see into, keep the
/// state where it is.
#[test]
fn what_cannot_be_proven_is_not_claimed() {
    for f in ["two_branches.tsx", "unresolved_child.tsx"] {
        assert!(findings(f, &[]).is_empty(), "{f}: {:#?}", findings(f, &[]));
    }
}

/// A provider is followed to the consumers of its context, and the state
/// moves with it; an element the analysis cannot see into, under the
/// provider, may be a consumer too (#145).
#[test]
fn a_state_reaching_its_consumer_by_context_is_followed() {
    let ds = findings("context.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    let m = message(&ds[0]);
    assert!(m.contains("`<Search>`, 2 levels below `App`"), "{m}");
    assert!(m.contains("with the `Query` provider"), "{m}");
}

/// List items are many instances: the descent stops at the component that
/// maps them, never inside one item.
#[test]
fn list_items_keep_the_state_in_the_component_that_maps_them() {
    let ds = findings("list_items.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(message(&ds[0]).contains("`<Wrapper>`, 1 level below `App`"));
}

/// A writer that also writes a module binding changes whatever reads it: a
/// sibling of the state's reader that does keeps the home at the owner
/// (#147).
#[test]
fn a_module_binding_written_by_the_writer_is_a_use_where_it_is_read() {
    let ds = findings("module_write.tsx", &[]);
    let ms: Vec<&str> = ds.iter().map(message).collect();
    assert_eq!(ms.len(), 2, "{ms:#?}");
    assert!(
        ms.iter()
            .any(|m| m.contains("`<PlainField>`, 2 levels below `Plain`")),
        "{ms:#?}"
    );
    // A utility inlined into the writer binds its result locally: no module
    // write, so the opaque sibling is not a possible reader of one.
    assert!(
        ms.iter()
            .any(|m| m.contains("`<InlinedField>`, 2 levels below `Inlined`")),
        "{ms:#?}"
    );
}

#[test]
fn a_bad_option_is_a_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args([
            "check",
            "tests/fixtures/state_lifted_too_high/drill.tsx",
            "--rule-option",
            "state-lifted-too-high:depth=2",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("has no option `depth`"));
}
