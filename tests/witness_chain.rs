//! ADR-019 end-to-end: typed witness chains with cross-file file identity.
//!
//! The flagship scenario: `useState(loadPrefs("theme"))` where `loadPrefs`
//! is imported from another file and its body calls `fetch`. The lazy-init
//! diagnostic must carry a `Resolve` step (import target = prefs.ts) and a
//! `Call` step whose span's `FileId` resolves to prefs.ts — the position the
//! ADR-011 limitation used to mis-attribute.

use std::path::PathBuf;

use reactant::{
    engine::{Config, RootStrategy},
    resolver::{DefaultImportResolver, analyze_lowered, lower_files},
    rules::{EffectClass, ResolveTarget, Rule, Step, lazy_init::LazyInit},
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/witness_chain")
        .join(name)
}

#[test]
fn lazy_init_witness_resolves_cross_file_effectful_call() {
    let files = vec![fixture("Settings.tsx"), fixture("prefs.ts")];
    let lowered = lower_files(&files, &DefaultImportResolver);
    assert!(lowered.parse_errors.is_empty(), "fixture must parse");
    let program = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());

    let diags = LazyInit.check(&program, &"Settings".to_string());
    assert_eq!(diags.len(), 1, "one lazy-init finding");
    let d = &diags[0];

    // Step 1: `loadPrefs` resolves to an import from prefs.ts.
    let resolve = d
        .notes
        .iter()
        .find_map(|n| match &n.step {
            Step::Resolve { name, target } if name == "loadPrefs" => Some(target),
            _ => None,
        })
        .expect("Resolve step for `loadPrefs`");
    match resolve {
        ResolveTarget::Import(p) => {
            assert!(
                p.ends_with("prefs.ts"),
                "resolved to prefs.ts, got {}",
                p.display()
            );
        }
        other => panic!("expected Import target, got {other:?}"),
    }

    // Step 2: the resolved body's `fetch` call, classified effectful, with a
    // span whose FileId resolves to prefs.ts (cross-file — ADR-011 is gone).
    let call_note = d
        .notes
        .iter()
        .find(|n| {
            matches!(
                &n.step,
                Step::Call { callee, class: EffectClass::Effectful } if callee == "fetch"
            )
        })
        .expect("Call step for `fetch`");
    let range = call_note.range.expect("fetch call carries a span");
    let file = program
        .file_table
        .path(range.file)
        .expect("FileId resolves in the program's FileTable");
    assert!(
        file.ends_with("prefs.ts"),
        "span points into prefs.ts, got {}",
        file.display()
    );
    assert_eq!(range.line, 2, "fetch is on line 2 of prefs.ts");

    // Rendered prose exists for every step (single rendering point).
    assert!(d.notes.iter().all(|n| !n.message.is_empty()));
}
