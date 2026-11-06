//! A finding has a position (#259).
//!
//! `missing-deps` rows came out with `line: null` whenever the memo or callback
//! was written as `return useMemo(…)`: `Terminator::Return` carried no span, so
//! the hook extracted from it had none. The JSON schema says `line` is an
//! integer, and the corpus baseline keys on it.

use std::path::Path;
use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .args(["--format", "json", "--fail-on", "never"])
        .args(["--config", "tests/fixtures/all-rules.json"])
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn diagnostics(path: &str) -> Vec<serde_json::Value> {
    let out = reactant(&[path]);
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{path}: stdout must be valid JSON: {e}"));
    doc["diagnostics"].as_array().expect("diagnostics").clone()
}

/// `useEvents` returns `useMemo(() => ({ events }), [handler])` — the memo is
/// reached only through the `return` of a hook defined in another file — and
/// `App` reads the destructured `events` in a memo that lists nothing.
#[test]
fn a_missing_deps_row_on_a_memo_returned_by_a_hook_has_a_line() {
    let rows: Vec<_> = diagnostics("tests/fixtures/positioned_hook_result")
        .into_iter()
        .filter(|d| d["rule"] == "missing-deps")
        .collect();
    let files: Vec<&str> = rows.iter().filter_map(|d| d["file"].as_str()).collect();
    assert!(
        files.iter().any(|f| f.ends_with("use-events.ts"))
            && files.iter().any(|f| f.ends_with("App.tsx")),
        "both memos must be reported: {rows:#?}"
    );
    for d in &rows {
        assert!(d["line"].is_u64(), "no line on {d:#}");
        assert!(d["col"].is_u64(), "no column on {d:#}");
    }
}

/// Every warning and error the fixtures produce is located. `Info` rows may
/// describe a limit of the analysis rather than a place in the source.
#[test]
fn every_fixture_finding_has_a_line() {
    let mut entries: Vec<_> = std::fs::read_dir("tests/fixtures")
        .expect("fixtures directory")
        .map(|e| e.expect("entry").path())
        .collect();
    entries.sort();
    for path in entries {
        // `all-rules.json` is the config; `packs` is a pack catalogue, not a
        // project to analyze.
        if path.extension().is_some_and(|e| e == "json") || path.ends_with("packs") {
            continue;
        }
        let path = path_str(&path);
        for d in diagnostics(&path) {
            assert!(
                d["severity"] == "info" || d["line"].is_u64(),
                "{path}: no line on {d:#}"
            );
        }
    }
}

fn path_str(p: &Path) -> String {
    p.to_str().expect("utf-8 path").to_string()
}
