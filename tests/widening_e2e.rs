//! End-to-end tests for threshold widening ("widening up-to", ADR-014).
//!
//! Full pipeline: source fixture → lowering → fixpoint → rule / converged state.
//!
//! Threshold widening recovers precision in the ascending phase by jumping a
//! growing bound to the tightest enclosing program constant instead of ±∞.
//!
//! `bounded_local_loop_is_precise` is the end-to-end witness for the *inner*
//! threshold widening (on the `analyze_cfg` back-edge): the lowering models
//! assignments / updates to existing variables (`x = e`, `x++`, `s += 1`) as
//! `Stmt::Assign` (see `expr_lower.rs`), so a local loop counter grows in the IR
//! and converges to the threshold instead of freezing at its init value.

use std::collections::HashMap;

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::{Interval, StateValue, StateValueTransfer},
    engine::{AnalysisResult, Config, analyze_component},
    lowering::lower_program,
    rules::{AlwaysUnstableDeps, InfiniteLoop, Rule},
};

/// Lower the fixture and analyse every component intra-procedurally.
fn analyze_fixture() -> HashMap<String, AnalysisResult<StateValue>> {
    let src = std::fs::read_to_string("tests/fixtures/widening.tsx").expect("read fixture");
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, &src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let components = lower_program(
        &ret.program,
        &src,
        std::path::Path::new("widening.tsx"),
        &mut Default::default(),
    );
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            (
                name,
                analyze_component(comp, &StateValueTransfer, &Config::default()),
            )
        })
        .collect()
}

fn rule_hits<R: Rule>(
    rule: &R,
    results: &HashMap<String, AnalysisResult<StateValue>>,
    name: &str,
) -> usize {
    use reactant::engine::{ComponentCallGraph, ProgramAnalysisResult};
    let mut components = HashMap::new();
    components.insert(name.to_string(), results[name].clone());
    let prog = ProgramAnalysisResult {
        components,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
        file_table: Default::default(),
        function_registry: Default::default(),
    };
    rule.check(&prog, &name.to_string()).len()
}

fn infinite_loop_hits(results: &HashMap<String, AnalysisResult<StateValue>>, name: &str) -> usize {
    rule_hits(&InfiniteLoop, results, name)
}

/// Abstract value of the first `useState` label (label 0) for `component`.
fn state0(results: &HashMap<String, AnalysisResult<StateValue>>, name: &str) -> StateValue {
    results[name].state_store.get(0)
}

#[test]
fn unbounded_counter_is_flagged_and_grows_to_infinity() {
    let r = analyze_fixture();
    assert_eq!(
        infinite_loop_hits(&r, "UnboundedCounter"),
        1,
        "unguarded self-increment must be flagged"
    );
    let v = state0(&r, "UnboundedCounter");
    assert!(
        !v.num.is_bottom() && v.num.hi.is_infinite(),
        "count must reach +∞ (got {v:?})"
    );
}

#[test]
fn guarded_counter_converges_bounded() {
    // Branch narrowing bounds the setter argument; threshold widening converges
    // without overshoot. (Convergence here is primarily branch narrowing —
    // pre-existing — but the test pins the combined behaviour.)
    let r = analyze_fixture();
    assert_eq!(
        infinite_loop_hits(&r, "GuardedCounter"),
        0,
        "guarded increment must converge, no infinite-loop"
    );
    assert_eq!(
        state0(&r, "GuardedCounter"),
        StateValue::number(Interval {
            lo: 0.0,
            hi: 10.0,
            is_int: true
        }),
        "guarded counter must converge to [0, 10]"
    );
    // Regression: `[count]` is a primitive (value-compared) dep, not a fresh
    // reference each render — must not trip always-unstable-deps even though
    // `count` converged to a wide interval.
    assert_eq!(
        rule_hits(&AlwaysUnstableDeps, &r, "GuardedCounter"),
        0,
        "numeric state dep must not be flagged always-unstable"
    );
}

#[test]
fn bounded_local_loop_is_precise() {
    let r = analyze_fixture();
    assert_eq!(infinite_loop_hits(&r, "BoundedLocalLoop"), 0);
    assert_eq!(
        state0(&r, "BoundedLocalLoop"),
        StateValue::number(Interval {
            lo: 0.0,
            hi: 5.0,
            is_int: true
        }),
        "loop counter bounded by guard constant 5 → setter writes total ∈ [0, 5]"
    );
}
