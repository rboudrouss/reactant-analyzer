//! The summary line may not claim a clean bill for code the analyzer never
//! read (#9, #47).
//!
//! Silence is only evidence when the analyzer looked. Before this, a Vite
//! project whose aliases live in `vite.config` — every import unresolved, every
//! target unlowered — ended with a green `✓ N file(s) no issues found.`, which
//! is the worst possible line on a tool whose claim is that false negatives are
//! forbidden.
//!
//! Drives the compiled binary: the summary line *is* the behaviour under test.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The control: a run with nothing unread still gets the green tick. Without
/// this the fix could "pass" by never claiming a clean bill at all.
#[test]
fn a_run_that_read_everything_keeps_its_clean_bill() {
    let out = reactant(&["tests/fixtures/clean.tsx"]);
    let s = stdout(&out);
    assert!(s.contains("no issues found"), "{s}");
    assert!(!s.contains("not analyzed:"), "{s}");
    assert_eq!(out.status.code(), Some(0));
}

/// Aliases the resolver cannot load: every `@/...` target is unlowered, so the
/// run has no basis for a clean bill.
#[test]
fn unloadable_aliases_withhold_the_clean_bill() {
    let out = reactant(&["tests/fixtures/blind_spots/vite_no_paths"]);
    let s = stdout(&out);
    assert!(!s.contains("no issues found"), "{s}");
    assert!(s.contains("not a clean bill"), "{s}");
    assert!(s.contains("no tsconfig `paths` found"), "{s}");
}

/// An import that resolved to a real file discovery never walked to. The
/// blind spot names the file, because naming it is the fix the user needs.
#[test]
fn an_import_resolved_outside_the_run_is_named() {
    let out = reactant(&["tests/fixtures/blind_spots/outside_root/app"]);
    let s = stdout(&out);
    assert!(!s.contains("no issues found"), "{s}");
    assert!(s.contains("useThing.ts"), "{s}");
}

/// …and the finding it was hiding is real: pass the directory above and the
/// `infinite-loop` shows up. The blind spot is not decoration.
#[test]
fn the_unread_import_was_hiding_a_finding() {
    let out = reactant(&["tests/fixtures/blind_spots/outside_root"]);
    let s = stdout(&out);
    assert!(s.contains("infinite-loop"), "{s}");
    assert!(!s.contains("not analyzed:"), "{s}");
}

/// A blind spot is not a finding: it says the counts are a lower bound, not
/// that something is wrong. The exit code keeps meaning "findings were
/// reported", so a CI pipeline's contract does not silently change.
#[test]
fn a_blind_spot_is_not_a_finding() {
    let out = reactant(&["tests/fixtures/blind_spots/vite_no_paths"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

/// "No components detected" takes its own early exit in the renderer, and it is
/// the shape this failure takes when the aliases the components hid behind
/// never loaded — so it has to withhold the tick too.
#[test]
fn no_components_detected_also_withholds_the_tick() {
    let out = reactant(&["tests/fixtures/blind_spots/vite_no_components"]);
    let s = stdout(&out);
    assert!(s.contains("no components detected"), "{s}");
    assert!(!s.contains("✓"), "{s}");
    assert!(s.contains("not analyzed:"), "{s}");
}

/// Machine consumers get the same fact, keyed, without scraping prose.
#[test]
fn json_carries_the_blind_spots() {
    let out = reactant(&[
        "tests/fixtures/blind_spots/outside_root/app",
        "--format",
        "json",
    ]);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("stdout must be valid JSON");
    let spots = doc["blind_spots"].as_array().expect("blind_spots array");
    assert_eq!(spots.len(), 1, "{spots:?}");
    assert_eq!(spots[0]["kind"], "unread-imports");
    assert_eq!(spots[0]["count"], 1);
}

/// The array is present and empty on a clean run, so a consumer can test it
/// unconditionally.
#[test]
fn json_blind_spots_is_empty_not_absent() {
    let out = reactant(&["tests/fixtures/clean.tsx", "--format", "json"]);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("stdout must be valid JSON");
    assert_eq!(doc["blind_spots"].as_array().expect("array").len(), 0);
}

/// With findings on the board the caveat still prints: the counts are a lower
/// bound, and that is worth the same sentence. The caveat block hangs off the
/// blind-spot list, not off whether the run happened to be silent.
#[test]
fn findings_and_blind_spots_are_reported_together() {
    let out = reactant(&["tests/fixtures/blind_spots/outside_root/app"]);
    let s = stdout(&out);
    assert!(s.contains("infinite-loop"), "{s}");
    assert!(s.contains("warning(s) across"), "{s}");
    assert!(s.contains("not analyzed:"), "{s}");
    assert!(s.contains("useThing.ts"), "{s}");
}
