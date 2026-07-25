//! conditional-hook end-to-end: void hooks (useEffect, useRef) must be
//! located at their real call-site block via `Expr::HookMarker`, not the
//! entry-block default that silently exempted them (FN). An early return
//! before the hooks makes every one of them conditional.

use reactant::rules::RuleCtx;
use std::path::PathBuf;

use reactant::{
    engine::{Config, RootStrategy},
    resolver::{DefaultImportResolver, analyze_lowered, lower_files},
    rules::{Rule, conditional_hook::ConditionalHook},
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conditional_hook")
        .join(name)
}

#[test]
fn void_hooks_after_early_return_are_flagged() {
    let files = vec![fixture("EarlyReturn.tsx")];
    let lowered = lower_files(&files, &DefaultImportResolver);
    assert!(lowered.parse_errors.is_empty(), "fixture must parse");
    let program = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());

    let diags = ConditionalHook.check(&RuleCtx::new(&program, &"EarlyReturn".to_string()));

    // useEffect + useRef + useMemo after the early return; useState before it
    // is legal, and the event handler must not be flagged (not a hook).
    assert_eq!(
        diags.len(),
        3,
        "effect, ref and memo are conditional: {diags:#?}"
    );

    // Every finding points its guard note at the early-return condition.
    for d in &diags {
        let note = d.notes.first().expect("guard note present");
        let range = note.range.expect("guard note carries a location");
        assert_eq!(range.line, 6, "guard is the `kind === \"special\"` test");
    }
}
