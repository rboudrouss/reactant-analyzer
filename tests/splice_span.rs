//! #131 — the splice synthesises two kinds of statement, and neither had a
//! position.
//!
//! Grafting a callee into a caller turns its `Return(e)` into an assignment and
//! prepends `let param = arg;` bindings. Both execute at the call site — that
//! is what inlining means — but they were minted with no span, so every finding
//! a rule anchored on one rendered with no line number, and `#129`'s location
//! grouping could not collapse them either. In the corpus that was six
//! `commerce` components reporting a `JSON.stringify` they do not contain, at
//! no position at all.

use std::process::{Command, Output};

const FIXTURE: &str = "tests/fixtures/splice_span";

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

#[test]
fn a_call_spliced_in_from_a_utilitys_return_reports_the_call_site() {
    let out = String::from_utf8_lossy(
        &reactant(&["check", FIXTURE, "--fail-on", "never", "--info"]).stdout,
    )
    .into_owned();
    assert!(
        out.contains("probe/render-call"),
        "the probe must see the spliced `JSON.parse`:\n{out}"
    );
    assert!(
        out.contains("(line 4:8)"),
        "`const data = normalize(raw);` is App.tsx line 4 — the one position \
         the splice knows, since `Terminator::Return` carries none:\n{out}"
    );
}
