//! End-to-end tests for the `derived-state` rule.
//!
//! Pattern: `useEffect(() => setB(expr), [stateA])` where `expr` is call-free
//! and `setB` is not called anywhere else.  Should be replaced by `useMemo` or
//! inlined into the render body.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{DerivedState, Rule},
};

fn make_prog(
    name: &str,
    result: reactant::engine::AnalysisResult<reactant::domains::StateValue>,
) -> reactant::engine::ProgramAnalysisResult {
    let mut components = std::collections::HashMap::new();
    components.insert(name.to_string(), result);
    reactant::engine::ProgramAnalysisResult {
        components,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: reactant::engine::ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
    }
}

fn run(src: &str) -> Vec<reactant::engine::AnalysisResult<reactant::domains::StateValue>> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts);
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .map(|comp| analyze_component(comp, &StateValueTransfer, &Config::default()))
        .collect()
}

fn derived_state_hits(src: &str) -> usize {
    let alloc = oxc_allocator::Allocator::default();
    let ret = oxc_parser::Parser::new(&alloc, src, oxc_span::SourceType::tsx())
        .with_options(oxc_parser::ParseOptions::default())
        .parse();
    let line_starts = reactant::lowering::compute_line_starts(src);
    let components = reactant::lowering::lower_program(&ret.program, &line_starts);
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = reactant::engine::analyze_component(
                comp,
                &reactant::domains::StateValueTransfer,
                &reactant::engine::Config::default(),
            );
            let prog = make_prog(&name, result);
            DerivedState.check(&prog, &name).len()
        })
        .sum()
}

// ── True positives ────────────────────────────────────────────────────────────

#[test]
fn arithmetic_derivation_fires() {
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [a, setA] = useState(0);
            const [b, setB] = useState(0);
            useEffect(() => { setB(a + 1); }, [a]);
            return <div>{a} {b}</div>;
        }
        "#,
    );
    assert_eq!(hits, 1, "simple a+1 derivation must fire");
}

#[test]
fn identity_derivation_fires() {
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [src, setSrc] = useState(0);
            const [copy, setCopy] = useState(0);
            useEffect(() => { setCopy(src); }, [src]);
            return <div>{src} {copy}</div>;
        }
        "#,
    );
    assert_eq!(hits, 1, "identity setCopy(src) must fire derived-state");
}

#[test]
fn unary_negation_derivation_fires() {
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [on, setOn] = useState(false);
            const [off, setOff] = useState(true);
            useEffect(() => { setOff(!on); }, [on]);
            return <div>{on ? "on" : "off"}</div>;
        }
        "#,
    );
    assert_eq!(hits, 1, "!on derivation must fire");
}

// ── True negatives ────────────────────────────────────────────────────────────

#[test]
fn call_in_arg_does_not_fire() {
    // Math.abs(a) is a Call → not call-free → must not fire
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [a, setA] = useState(0);
            const [b, setB] = useState(0);
            useEffect(() => { setB(Math.abs(a)); }, [a]);
            return <div>{a} {b}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "call in arg must not fire derived-state");
}

#[test]
fn two_deps_does_not_fire() {
    // Two deps → not a single-source derivation
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [a, setA] = useState(0);
            const [c, setC] = useState(0);
            const [b, setB] = useState(0);
            useEffect(() => { setB(a + c); }, [a, c]);
            return <div>{a} {b} {c}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "two deps must not fire derived-state");
}

#[test]
fn no_deps_does_not_fire() {
    // No dep array → ambiguous origin, skip
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [a, setA] = useState(0);
            const [b, setB] = useState(0);
            useEffect(() => { setB(a + 1); });
            return <div>{a} {b}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "no deps array must not fire derived-state");
}

#[test]
fn setter_also_in_render_does_not_fire() {
    // setB called both in effect and render → not a pure derivation
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [a, setA] = useState(0);
            const [b, setB] = useState(0);
            setB(0); // also called here
            useEffect(() => { setB(a + 1); }, [a]);
            return <div>{a} {b}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "setter in render must prevent derived-state");
}

#[test]
fn empty_deps_does_not_fire() {
    // [] deps → runs once on mount only → not a reactive derivation
    let hits = derived_state_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [a, setA] = useState(0);
            const [b, setB] = useState(0);
            useEffect(() => { setB(a + 1); }, []);
            return <div>{a} {b}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "empty deps must not fire derived-state");
}

#[test]
fn clean_component_no_false_positive() {
    // No effect at all → no derived-state
    let hits = derived_state_hits(
        r#"
        import { useState } from "react";
        function C() {
            const [count, setCount] = useState(0);
            return <button onClick={() => setCount(count + 1)}>{count}</button>;
        }
        "#,
    );
    assert_eq!(hits, 0, "clean component must have 0 derived-state hits");
}

// ── Fixture regression ────────────────────────────────────────────────────────

#[test]
fn derived_state_fixture() {
    let src = std::fs::read_to_string("tests/fixtures/derived_state.tsx")
        .expect("derived_state.tsx not found");
    let hits = derived_state_hits(&src);
    // DerivedArith (a+1) + DerivedField (user.name) = 2 true positives.
    // All others are false negatives by design (non-state dep, two deps, call in arg, setter in render, clean).
    assert_eq!(
        hits, 2,
        "derived_state.tsx: expected exactly 2 derived-state hits"
    );
}
