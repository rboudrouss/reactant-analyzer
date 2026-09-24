//! End-to-end tests for `wasted-subtree-render`: a state written from a
//! continuous event, in a component that also builds elements none of whose
//! inputs depend on it.
//!
//! `typing`, `composer`, `dropzone`, `click` and `children` come from
//! `scripts/rerender-bench`, where their re-render counts were measured at
//! runtime (React 19, jsdom).

use std::process::Command;

const RULE: &str = "wasted-subtree-render";

fn findings(fixture: &str, extra: &[&str]) -> Vec<serde_json::Value> {
    let path = format!("tests/fixtures/wasted_subtree_render/{fixture}");
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

fn message(d: &serde_json::Value) -> String {
    d["message"].as_str().expect("message").to_string()
}

#[test]
fn typing_next_to_a_heavy_sibling() {
    let ds = findings("typing.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    let m = message(&ds[0]);
    assert!(m.contains("each `change` event writes state `text`"), "{m}");
    assert!(m.contains("`<ExpensiveTree>`"), "{m}");
    assert!(!m.contains("Preview"), "Preview reads `text`: {m}");
    assert!(findings("typing_fixed.tsx", &[]).is_empty());
}

/// One handler writing two slots is one trigger: one finding, and only the
/// elements that depend on neither.
#[test]
fn slots_written_together_are_one_trigger() {
    let ds = findings("composer.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    let m = message(&ds[0]);
    assert!(m.contains("states `value` and `isCommentEmpty`"), "{m}");
    assert!(
        m.contains("`<AttachmentPicker>` and `<EmojiPickerButton>`"),
        "{m}"
    );
    assert!(
        !m.contains("SendButton") && !m.contains("Suggestions"),
        "{m}"
    );
}

#[test]
fn a_listener_registered_in_an_effect_is_a_trigger() {
    let ds = findings("pointer.tsx", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(message(&ds[0]).contains("each `mousemove` event"));
}

/// Clicks, checkbox toggles and drag crossings are discrete: reported only
/// when asked.
#[test]
fn discrete_triggers_need_the_option() {
    for f in ["click.tsx", "checkbox.tsx", "dropzone.tsx"] {
        assert!(findings(f, &[]).is_empty(), "{f}");
        // `click.tsx` has two triggers (the button and the dialog's `onClose`).
        let ds = findings(
            f,
            &[
                "--rule-option",
                "wasted-subtree-render:continuousOnly=false",
            ],
        );
        assert!(!ds.is_empty(), "{f}");
    }
}

/// A continuous event writing a boolean re-renders only when the boolean
/// flips: discrete by default.
#[test]
fn a_few_valued_state_is_not_continuous() {
    assert!(findings("scroll_flag.tsx", &[]).is_empty());
    let ds = findings(
        "scroll_flag.tsx",
        &[
            "--rule-option",
            "wasted-subtree-render:continuousOnly=false",
        ],
    );
    assert_eq!(ds.len(), 1, "{ds:#?}");
}

#[test]
fn children_from_above_and_provider_subtrees_are_not_claimed() {
    assert!(findings("children.tsx", &[]).is_empty());
    assert!(findings("provider.tsx", &[]).is_empty());
}

/// A trigger spliced in from a custom hook is the hook's to fix, and the
/// message says so.
#[test]
fn a_trigger_inside_a_custom_hook_names_the_hook() {
    let ds = findings("hook_trigger", &[]);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    let m = message(&ds[0]);
    assert!(m.contains("each `resize` event writes state `size`"), "{m}");
    assert!(m.contains("The writes come from `useWindowSize`"), "{m}");
}
