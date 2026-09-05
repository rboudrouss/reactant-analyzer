//! #110 — the persisted phase-1/phase-2 reachability split.
//!
//! `analyze_program` runs two phases: phase 1 analyses roots top-down with an
//! `InterCtx` (recording call-graph edges), phase 2 sweeps everything phase 1
//! did not reach, intra-only, with no `InterCtx`. A phase-2 component records
//! no edges, so through `callers_of` alone it is indistinguishable from a
//! genuine root — both answer "no callers".
//!
//! Every relation that walks ancestry to conclude an ABSENCE has to tell those
//! two apart, or it fails open on exactly the components whose parents it
//! cannot see.

use reactant::engine::{
    ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy, analyze_program,
};

fn analyze(src: &str, strategy: RootStrategy) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::lowering::lower_program;

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
    let components = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        strategy,
        &Config::default(),
    )
}

const SRC: &str = r#"
export function App() {
  return <Child n={1} />;
}
function Child({ n }) {
  return <div>{n}</div>;
}
function Widget() {
  return <Leaf/>;
}
function Leaf() {
  return <span/>;
}
"#;

fn sym(s: &str) -> String {
    s.to_string()
}

/// `--entry App`: `Widget` and `Leaf` are never reached top-down, and `Leaf`
/// is referenced, so it is not a heuristic root either — the phase-2 shape.
fn only_app() -> ProgramAnalysisResult {
    analyze(SRC, RootStrategy::Explicit(vec![sym("App")]))
}

#[test]
fn a_component_reached_top_down_is_in_the_set() {
    let prog = only_app();
    assert!(
        prog.was_inter_analyzed(prog.component_named("App").unwrap()),
        "the root is phase 1"
    );
    assert!(
        prog.was_inter_analyzed(prog.component_named("Child").unwrap()),
        "a component reached from a root is phase 1: {:?}",
        prog.phase1_reached
    );
}

#[test]
fn a_phase_two_only_component_is_not_in_the_set() {
    let prog = only_app();
    assert!(
        prog.component_named("Leaf").is_some(),
        "phase 2 still analyses it"
    );
    assert!(
        !prog.was_inter_analyzed(prog.component_named("Leaf").unwrap()),
        "nothing reached it top-down: {:?}",
        prog.phase1_reached
    );
}

#[test]
fn the_split_separates_a_root_from_an_unreached_component() {
    // The whole point: `callers_of` cannot tell these two apart.
    let prog = only_app();
    assert!(
        prog.call_graph
            .callers_of(prog.component_named("App").unwrap())
            .is_empty()
    );
    assert!(
        prog.call_graph
            .callers_of(prog.component_named("Leaf").unwrap())
            .is_empty(),
        "a phase-2 component records no edges, so it reads as caller-less"
    );
    assert_ne!(
        prog.was_inter_analyzed(prog.component_named("App").unwrap()),
        prog.was_inter_analyzed(prog.component_named("Leaf").unwrap()),
        "the persisted split is what separates them"
    );
}

#[test]
fn ancestry_is_unknown_rather_than_empty_for_an_unreached_component() {
    let prog = only_app();
    assert_eq!(
        prog.complete_ancestry(prog.component_named("App").unwrap()),
        Some(Default::default()),
        "a genuine root has a complete, empty ancestry"
    );
    assert_eq!(
        prog.complete_ancestry(prog.component_named("Leaf").unwrap()),
        None,
        "an unreached component's ancestry is unknown, not empty"
    );
    let child = prog
        .complete_ancestry(prog.component_named("Child").unwrap())
        .expect("Child's whole chain is inter-analysed");
    assert!(
        child.contains(&prog.component_named("App").unwrap()),
        "{child:?}"
    );
}

#[test]
fn a_chain_through_an_unreached_parent_has_no_complete_ancestry() {
    // `Leaf` is reached, but only from `Widget`, which nothing reached. Its
    // ancestry is therefore incomplete even though it has a known caller.
    let prog = analyze(SRC, RootStrategy::Explicit(vec![sym("App"), sym("Leaf")]));
    assert!(prog.was_inter_analyzed(prog.component_named("Leaf").unwrap()));
    assert!(
        !prog.was_inter_analyzed(prog.component_named("Widget").unwrap()),
        "Widget is never reached: {:?}",
        prog.phase1_reached
    );
}

#[test]
fn every_component_is_phase_one_under_all_components() {
    // The split is a property of the strategy, not a constant.
    let prog = analyze(SRC, RootStrategy::AllComponents);
    for name in prog.components.keys() {
        assert!(prog.was_inter_analyzed(*name), "{name:?} should be phase 1");
    }
}

#[test]
fn a_reference_cycle_leaves_both_components_unreached() {
    // Neither is a heuristic root (both are referenced), so phase 1 analyses
    // nothing and both fall to the intra-only sweep. `callers_of` is empty for
    // both, and only the persisted set says why.
    let prog = analyze(
        r#"
function Ping() { return <Pong/>; }
function Pong() { return <Ping/>; }
"#,
        RootStrategy::Heuristic,
    );
    for name in ["Ping", "Pong"] {
        assert!(
            !prog.was_inter_analyzed(prog.component_named(name).unwrap()),
            "{name} was never reached top-down: {:?}",
            prog.phase1_reached
        );
        assert_eq!(
            prog.complete_ancestry(prog.component_named(name).unwrap()),
            None
        );
    }
}
