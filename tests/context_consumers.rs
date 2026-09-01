//! #115 — the `context_consumers` relation, and the two gates that stop it
//! failing open.
//!
//! The verdict is an ABSENCE ("nothing above this consumer provides the
//! context"), so it is only as trustworthy as the paths the analysis could
//! complete. Phase 2 analyses everything phase 1 did not reach, intra-only,
//! recording no call-graph edges — so an unreached component reads as a
//! caller-less root, and reading that as "no ancestors" fires the rule on every
//! consumer whose real parent was never entered.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::engine::{Config, RootStrategy};
use reactant::resolver::{DefaultImportResolver, analyze_lowered, lower_files};
use reactant::rules::declarative::load_pack;
use reactant::rules::{Diagnostic, RuleCtx};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const PACK: &str = r#"{
  "schemaVersion": 1, "name": "cc",
  "rules": [{
    "id": "no-provider",
    "docs": {"description":"d","why":"w","fix":"f"},
    "severity": "warning",
    "anchor": { "relation": "context_consumers" },
    "guards": [{ "kind": "provider", "of": "anchor", "is": ["none-on-analyzed-paths"] }],
    "message": "reads {anchor.name}"
  }]
}"#;

/// Run `PACK` over `files`, analysed with `strategy`.
fn run(files: &[(&str, &str)], strategy: RootStrategy) -> Vec<Diagnostic> {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("reactant-cc-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("tmp dir");
    let paths: Vec<PathBuf> = files
        .iter()
        .map(|(rel, src)| {
            let p = dir.join(rel);
            fs::write(&p, src).unwrap();
            p
        })
        .collect();

    let lowered = lower_files(&paths, &DefaultImportResolver::default());
    assert!(
        lowered.parse_errors.is_empty(),
        "{:?}",
        lowered.parse_errors
    );
    let prog = analyze_lowered(lowered, strategy, Config::default());
    let pack = load_pack(PACK, &BTreeMap::new()).expect("pack loads");

    let mut names: Vec<String> = prog.components.keys().cloned().collect();
    names.sort();
    let out: Vec<Diagnostic> = names
        .iter()
        .flat_map(|n| {
            let ctx = RuleCtx::new(&prog, n);
            pack.rules
                .iter()
                .flat_map(|r| r.rule.check(&ctx))
                .collect::<Vec<_>>()
        })
        .collect();
    let _ = fs::remove_dir_all(&dir);
    out
}

const CTX: (&str, &str) = (
    "ctx.ts",
    "import { createContext } from \"react\";\nexport const ThemeContext = createContext(null);\n",
);
const PANEL: (&str, &str) = (
    "panel.tsx",
    "import { useContext } from \"react\";\nimport { ThemeContext } from \"./ctx\";\nexport function Panel() {\n  const theme = useContext(ThemeContext);\n  return <div>{theme}</div>;\n}\n",
);
const APP_PLAIN: (&str, &str) = (
    "app.tsx",
    "import { Panel } from \"./panel\";\nexport function App() {\n  return <div><Panel/></div>;\n}\n",
);
const APP_PROVIDING: (&str, &str) = (
    "app.tsx",
    "import { ThemeContext } from \"./ctx\";\nimport { Panel } from \"./panel\";\nexport function App() {\n  return <ThemeContext.Provider value={\"dark\"}><Panel/></ThemeContext.Provider>;\n}\n",
);

#[test]
fn a_consumer_with_no_provider_above_it_fires() {
    let got = run(&[CTX, APP_PLAIN, PANEL], RootStrategy::AllComponents);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].message.contains("`ThemeContext`"),
        "{}",
        got[0].message
    );
}

#[test]
fn a_provider_on_an_ancestor_silences_it() {
    let got = run(&[CTX, APP_PROVIDING, PANEL], RootStrategy::AllComponents);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn the_provider_pairs_across_files_through_the_canonical_identity() {
    // The provider names the cell `Theme`, the consumer names it
    // `ThemeContext`. Only the canonical `ContextId` (#109) pairs them; on the
    // local name they would look like two different contexts and the rule
    // would fire.
    let aliased_app = (
        "app.tsx",
        "import { ThemeContext as Theme } from \"./ctx\";\nimport { Panel } from \"./panel\";\nexport function App() {\n  return <Theme.Provider value={\"dark\"}><Panel/></Theme.Provider>;\n}\n",
    );
    let got = run(&[CTX, aliased_app, PANEL], RootStrategy::AllComponents);
    assert!(got.is_empty(), "aliases name the same cell: {got:?}");
}

// ── The two gates ────────────────────────────────────────────────────────────

#[test]
fn gate_one_drops_a_consumer_that_was_never_reached_top_down() {
    // `--entry App` with `App` in a file that does NOT render Panel: Panel is
    // analysed intra-only, so nothing is known about what renders it. An empty
    // `callers_of` there means unknown ancestry, not "no ancestors".
    let unrelated_app = ("app.tsx", "export function App() {\n  return <div/>;\n}\n");
    let got = run(
        &[CTX, unrelated_app, PANEL],
        RootStrategy::Explicit(vec!["App".to_string()]),
    );
    assert!(
        got.is_empty(),
        "an unreached consumer has unknown ancestry, so no row: {got:?}"
    );
}

#[test]
fn gate_two_drops_a_consumer_whose_parent_is_a_phase_two_component() {
    // `--entry Panel`: Panel IS phase 1 and is caller-less, so gate 1 passes —
    // its ancestry looks complete and empty. But `App` was never entered and
    // syntactically renders `<Panel/>`, so it may be a parent holding the
    // provider. Only the syntactic completion pass catches this.
    let got = run(
        &[CTX, APP_PROVIDING, PANEL],
        RootStrategy::Explicit(vec!["Panel".to_string()]),
    );
    assert!(
        got.is_empty(),
        "an unreached component that renders the consumer may hold the provider: {got:?}"
    );
}

#[test]
fn gate_two_does_not_drop_a_row_no_unreached_component_mentions() {
    // The control for the test above: same shape, but the unreached component
    // does not render the consumer, so the ancestry really is complete.
    let unrelated = (
        "other.tsx",
        "export function Other() {\n  return <div/>;\n}\n",
    );
    let got = run(
        &[CTX, unrelated, PANEL],
        RootStrategy::Explicit(vec!["Panel".to_string()]),
    );
    assert_eq!(
        got.len(),
        1,
        "nothing unreached mentions Panel, so the row stands: {got:?}"
    );
}

#[test]
fn a_context_that_is_not_a_proven_cell_produces_no_row() {
    // `useContext(x)` where `x` is not a module-level `createContext` binding:
    // there is no cell to pair, so the relation says nothing rather than
    // guessing.
    let opaque = (
        "opaque.tsx",
        "import { useContext } from \"react\";\nimport { SomeCtx } from \"some-lib\";\nexport function P() {\n  const v = useContext(SomeCtx);\n  return <div>{v}</div>;\n}\n",
    );
    let got = run(&[opaque], RootStrategy::AllComponents);
    assert!(got.is_empty(), "a library context is unprovable: {got:?}");
}
