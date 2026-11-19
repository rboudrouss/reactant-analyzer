//! A write in a body the walk entered reads that body's names (#310).
//!
//! A write inside an IIFE, a helper or a callback is read at the call that
//! runs it, in the region's env, which holds the region's names only. A name
//! the entered body binds — a local, a parameter, a lowering temp, all of
//! which reuse names across bodies — was read as the region's binding of the
//! same name, and a counter read as a constant: the loop was reported clean.
//! Such a name is ⊤ now, and a body reached by name reads the names the
//! region rebinds as ⊤ too, since it captures where it is written.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, severity, message) the fixture reports.
fn findings() -> Vec<(String, String, String, String)> {
    let out = reactant(&[
        "tests/fixtures/shadowed_writes",
        "--config",
        "tests/fixtures/all-rules.json",
        "--format",
        "json",
        "--fail-on",
        "never",
    ]);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    doc["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| {
            let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
            (s("component"), s("rule"), s("severity"), s("message"))
        })
        .collect()
}

fn loop_level(all: &[(String, String, String, String)], component: &str) -> Option<String> {
    all.iter()
        .find(|(c, r, _, _)| c == component && r == "infinite-loop")
        .map(|(_, _, s, _)| s.clone())
}

/// Every one of these loops in React (oracle cases `shadow-*`): the write
/// stores a counter its body computes under a name another body binds too.
#[test]
fn a_write_reads_the_names_of_the_body_it_sits_in() {
    let all = findings();
    for component in [
        "TempInIife",
        "TempInNamedLocal",
        "TempInNestedClosures",
        "TempOfNullish",
        "TempOfAnd",
        "TempOfOptionalChain",
        "LocalInIife",
        "LocalInNamedHelper",
        "CallbackParam",
        "RenderNameByCallbackParam",
        "IifeParam",
        "NestedChain",
        "TimerClosure",
        "HelperCalledTwice",
        "RenderHelperLocal",
        "RenderHelperCapture",
        "UseCallbackLocal",
        "UseCallbackCapture",
        "UseCallbackRoot",
        "GuardOtherBinding",
    ] {
        assert!(
            loop_level(&all, component).is_some(),
            "{component}: the loop must be reported\n{all:#?}"
        );
    }
}

/// A name the entered body does not bind is the region's: the effect-local
/// counter an IIFE stores is still a certain loop.
#[test]
fn a_captured_name_of_the_region_keeps_its_value() {
    let all = findings();
    assert_eq!(
        loop_level(&all, "CaptureEffectLocal").as_deref(),
        Some("error"),
        "{all:#?}"
    );
}

/// No message prints a lowering temp.
#[test]
fn no_message_names_a_temp() {
    let all = findings();
    for (c, r, _, m) in &all {
        assert!(!m.contains("__"), "{c} {r}: {m}");
    }
}
